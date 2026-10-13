//! The Audeze app's PEQ DLL, used to build peak and pass filter banks.
//!
//! The DLL is never redistributed: Maxwell PEQ uses the copy the Audeze app
//! installs, and refuses any file whose hash differs from the inspected one.
//! It keeps global state, so every call holds one process-wide lock.

use crate::dsp::{Filter, Kind, MAX_BANDS, WORDS};
use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

pub const DLL_SHA256: &str = "322ac1149eeba9ed55f2466ba706edc92dcdb333cf574a6466abf88e4b3d50be";
pub const DLL_NAME: &str = "AirohaPeqLibrary.dll";

fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(directory) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        paths.push(directory.join(DLL_NAME));
    }
    let program_files =
        std::env::var_os("ProgramFiles").map_or_else(|| PathBuf::from("C:/Program Files"), PathBuf::from);
    paths.push(program_files.join("Audeze").join("AudezeApp").join(DLL_NAME));
    paths
}

/// `MAXWELL_PEQ_DLL`, else the first candidate that exists, else the Audeze app's path.
pub fn dll_path() -> PathBuf {
    if let Some(path) = std::env::var_os("MAXWELL_PEQ_DLL").filter(|path| !path.is_empty()) {
        let path = PathBuf::from(path);
        return std::fs::canonicalize(&path).unwrap_or(path);
    }
    let candidates = candidates();
    let found = candidates.iter().find(|path| path.is_file()).unwrap_or(&candidates[candidates.len() - 1]);
    std::fs::canonicalize(found).unwrap_or_else(|_| found.clone())
}

type Fingerprint = (PathBuf, u64, Option<SystemTime>);

/// The last successful hash check; status polls would otherwise rehash the DLL each time.
static VERIFIED: Mutex<Option<Fingerprint>> = Mutex::new(None);

pub fn verified_dll() -> Result<PathBuf> {
    if cfg!(not(windows)) {
        return Err(Error::value(
            "The Audeze PEQ library only exists in the Windows Audeze app; using the built-in encoder.",
        ));
    }
    let path = dll_path();
    let metadata = std::fs::metadata(&path).ok().filter(|m| m.is_file()).ok_or_else(|| {
        Error::value(
            "AirohaPeqLibrary.dll was not found. Install the Audeze app, or set MAXWELL_PEQ_DLL to the inspected DLL.",
        )
    })?;
    let fingerprint = (path.clone(), metadata.len(), metadata.modified().ok());
    let mut verified = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    if verified.as_ref() != Some(&fingerprint) {
        let bytes = std::fs::read(&path)?;
        if hex::encode(Sha256::digest(&bytes)) != DLL_SHA256 {
            return Err(Error::value("The PEQ DLL differs from the inspected version; compilation is disabled."));
        }
        *verified = Some(fingerprint);
    }
    Ok(path)
}

type SetParam = unsafe extern "C" fn(i32, f64, i32, i32, i32, i32);
type SetPeqPoint = unsafe extern "C" fn(i32, i32, f64, f64, f64);
type SetXpfPoint = unsafe extern "C" fn(i32, i32, f64, f64, i32);
type Generate = unsafe extern "C" fn(i32);
type Rescale = unsafe extern "C" fn(i32, f64);
type Count = unsafe extern "C" fn(i32) -> u16;
type Param = unsafe extern "C" fn(i32) -> *const u16;

/// A loaded, hash-verified DLL. Obtain it through [`with_vendor`].
pub struct Vendor {
    path: PathBuf,
    library: libloading::Library,
}

static LOADED: Mutex<Option<Vendor>> = Mutex::new(None);

/// Run `f` with the verified DLL, loading it on first use and keeping it loaded.
pub fn with_vendor<T>(f: impl FnOnce(&Vendor) -> Result<T>) -> Result<T> {
    let path = verified_dll()?;
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if loaded.as_ref().is_none_or(|vendor| vendor.path != path) {
        // SAFETY: the file's hash matches the inspected DLL, whose initializers
        // only set up its own tables.
        let library = unsafe { libloading::Library::new(&path) }
            .map_err(|error| Error::other(format!("Could not load {DLL_NAME}: {error}")))?;
        *loaded = Some(Vendor { path, library });
    }
    f(loaded.as_ref().expect("loaded above"))
}

impl Vendor {
    fn symbol<T: Copy>(&self, name: &[u8]) -> Result<T> {
        // SAFETY: the signatures above match the inspected DLL's exports.
        unsafe { self.library.get::<T>(name) }
            .map(|symbol| *symbol)
            .map_err(|error| Error::other(format!("{DLL_NAME} is missing an export: {error}")))
    }

    /// Low-level calls, for tests that compare against the DLL directly.
    pub fn set_param(&self, rate: f64) -> Result<()> {
        let f: SetParam = self.symbol(b"setParam\0")?;
        unsafe { f(0, rate, MAX_BANDS as i32, 1, 0, 0) };
        Ok(())
    }

    pub fn set_peq_point(&self, index: i32, frequency: f64, gain: f64, width: f64) -> Result<()> {
        let f: SetPeqPoint = self.symbol(b"set_peq_point\0")?;
        unsafe { f(0, index, frequency, gain, width) };
        Ok(())
    }

    pub fn set_xpf_point(&self, index: i32, frequency: f64, q: f64, kind: i32) -> Result<()> {
        let f: SetXpfPoint = self.symbol(b"set_xpf_point\0")?;
        unsafe { f(0, index, frequency, q, kind) };
        Ok(())
    }

    /// Generate, rescale by `preamp` dB, and copy out the coefficient words.
    pub fn finish(&self, preamp: f64) -> Result<Vec<u16>> {
        let generate: Generate = self.symbol(b"generate_cofe\0")?;
        let rescale: Rescale = self.symbol(b"change_rescale_cofe\0")?;
        let count: Count = self.symbol(b"get_cofe_count\0")?;
        let param: Param = self.symbol(b"get_cofe_param\0")?;
        unsafe {
            generate(0);
            rescale(0, preamp);
            let count = count(0);
            if usize::from(count) != WORDS {
                return Err(Error::value(format!("Unexpected coefficient count: {count}.")));
            }
            let words = param(0);
            if words.is_null() {
                return Err(Error::other("The PEQ DLL returned no coefficients."));
            }
            Ok(std::slice::from_raw_parts(words, WORDS).to_vec())
        }
    }

    pub fn destroy(&self) -> Result<()> {
        let f: Generate = self.symbol(b"destroy\0")?;
        unsafe { f(0) };
        Ok(())
    }

    /// One rate's 103 words for peak and pass filters.
    pub fn bank(&self, filters: &[Filter], preamp: f64, rate: f64) -> Result<Vec<u16>> {
        self.set_param(rate)?;
        let result = (|| {
            let flat = Filter::peak(1000.0, 0.0, 1.0);
            for i in 0..MAX_BANDS {
                let f = filters.get(i).filter(|f| f.enabled).unwrap_or(&flat);
                let index = i as i32;
                match f.kind {
                    Kind::LowPass => self.set_xpf_point(index, f.frequency, f.q, 1)?,
                    Kind::HighPass => self.set_xpf_point(index, f.frequency, f.q, 2)?,
                    // Static evidence: alpha = sin(omega) / (2 * (frequency / width)).
                    Kind::Peak => self.set_peq_point(index, f.frequency, f.gain, f.frequency / f.q)?,
                    Kind::LowShelf | Kind::HighShelf => {
                        return Err(Error::value("Shelf filters require the extended encoder."));
                    }
                }
            }
            self.finish(preamp)
        })();
        self.destroy()?;
        result
    }
}
