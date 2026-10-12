use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Lossless cwd representation; platform-specific units are never converted lossily.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "encoding", content = "value", rename_all = "snake_case")]
pub enum NativePath {
    Utf8(String),
    UnixBytes(Vec<u8>),
    WindowsWide(Vec<u16>),
}

impl NativePath {
    pub fn from_path(path: &Path) -> Self {
        if let Some(text) = path.to_str() {
            return Self::Utf8(text.to_owned());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Self::UnixBytes(path.as_os_str().as_bytes().to_vec())
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Self::WindowsWide(path.as_os_str().encode_wide().collect())
        }
        #[cfg(not(any(unix, windows)))]
        unreachable!("native paths require Unix or Windows")
    }

    pub fn requires_native(&self) -> bool {
        !matches!(self, Self::Utf8(_))
    }

    pub fn to_path(&self) -> Result<PathBuf, String> {
        match self {
            Self::Utf8(text) => Ok(PathBuf::from(text)),
            #[cfg(unix)]
            Self::UnixBytes(bytes) => {
                use std::os::unix::ffi::OsStringExt;
                Ok(std::ffi::OsString::from_vec(bytes.clone()).into())
            }
            #[cfg(windows)]
            Self::WindowsWide(units) => {
                use std::os::windows::ffi::OsStringExt;
                Ok(std::ffi::OsString::from_wide(units).into())
            }
            _ => Err("cwd native encoding belongs to another platform; refusing execution".into()),
        }
    }
}

pub fn legacy_is_exact(text: &str) -> bool {
    !text.is_empty() && !text.contains('\u{fffd}')
}

/// Legacy display strings containing replacements cannot prove which directory was used.
pub fn resolve(native: Option<&NativePath>, display: &str) -> Result<PathBuf, String> {
    match native {
        Some(path) => path.to_path(),
        None if legacy_is_exact(display) => Ok(PathBuf::from(display)),
        None => Err("ambiguous legacy cwd: original native path was not retained; start a new watch from the intended directory; path bytes will not be guessed".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_ambiguity_and_explicit_unicode_are_distinct() {
        assert!(resolve(None, "/ordinary").is_ok());
        assert!(resolve(None, "").is_err());
        assert!(resolve(None, "/replacement-�").is_err());
        let explicit = NativePath::Utf8("/replacement-�".into());
        assert_eq!(
            resolve(Some(&explicit), "ignored").unwrap(),
            Path::new("/replacement-�")
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_bytes_round_trip_without_aliasing_replacement_sibling() {
        use std::os::unix::ffi::OsStringExt;
        let original = PathBuf::from(std::ffi::OsString::from_vec(b"/raw-\xff".to_vec()));
        let native = NativePath::from_path(&original);
        let encoded = serde_json::to_vec(&native).unwrap();
        let decoded: NativePath = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.to_path().unwrap(), original);
        assert_ne!(
            decoded.to_path().unwrap(),
            PathBuf::from(original.to_string_lossy().as_ref())
        );
        assert!(NativePath::WindowsWide(vec![0xd800]).to_path().is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_units_round_trip_without_replacing_surrogates() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let units = [67, 58, 92, 0xd800];
        let path = PathBuf::from(std::ffi::OsString::from_wide(&units));
        let native: NativePath =
            serde_json::from_slice(&serde_json::to_vec(&NativePath::from_path(&path)).unwrap())
                .unwrap();
        assert_eq!(
            native
                .to_path()
                .unwrap()
                .as_os_str()
                .encode_wide()
                .collect::<Vec<_>>(),
            units
        );
        assert!(NativePath::UnixBytes(vec![255]).to_path().is_err());
    }
}
