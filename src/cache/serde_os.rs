use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug)]
pub(super) struct StoredOsString(pub(super) OsString);

impl Serialize for StoredOsString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SerializableOsStr(&self.0).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StoredOsString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_os(deserializer).map(Self)
    }
}

struct SerializableOsStr<'a>(&'a OsStr);

impl Serialize for SerializableOsStr<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(text) = self.0.to_str() {
            return serializer.serialize_str(text);
        }
        EncodedOsValue {
            display: self.0.to_string_lossy().into_owned(),
            encoding: host_encoding(),
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(os_bytes(self.0)),
        }
        .serialize(serializer)
    }
}

#[derive(Serialize)]
struct EncodedOsValue {
    display: String,
    encoding: &'static str,
    bytes_base64: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OsValue {
    Text(String),
    Encoded {
        #[allow(dead_code)]
        display: String,
        encoding: String,
        bytes_base64: String,
    },
}

fn deserialize_os<'de, D>(deserializer: D) -> Result<OsString, D::Error>
where
    D: Deserializer<'de>,
{
    let value = OsValue::deserialize(deserializer)?;
    match value {
        OsValue::Text(text) => Ok(OsString::from(text)),
        OsValue::Encoded {
            encoding,
            bytes_base64,
            ..
        } => {
            if encoding != host_encoding() {
                return Err(de::Error::custom(format!(
                    "cache path uses unsupported `{encoding}` encoding"
                )));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(bytes_base64)
                .map_err(de::Error::custom)?;
            os_from_bytes(bytes).map_err(de::Error::custom)
        }
    }
}

pub(super) mod path {
    use super::*;

    pub(crate) fn serialize<S>(value: &Path, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SerializableOsStr(value.as_os_str()).serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_os(deserializer).map(PathBuf::from)
    }
}

pub(super) mod option_path {
    use super::*;

    pub(crate) fn serialize<S>(value: &Option<PathBuf>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(path) => serializer.serialize_some(&SerializableOsStr(path.as_os_str())),
            None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Option<PathBuf>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OptionPathVisitor;

        impl<'de> Visitor<'de> for OptionPathVisitor {
            type Value = Option<PathBuf>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a path or null")
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(None)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserialize_os(deserializer).map(|path| Some(PathBuf::from(path)))
            }
        }

        deserializer.deserialize_option(OptionPathVisitor)
    }
}

#[cfg(unix)]
fn host_encoding() -> &'static str {
    "unix-bytes"
}

#[cfg(windows)]
fn host_encoding() -> &'static str {
    "windows-wide-le"
}

#[cfg(not(any(unix, windows)))]
fn host_encoding() -> &'static str {
    "utf-8"
}

#[cfg(unix)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;

    value.as_bytes().to_vec()
}

#[cfg(windows)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt as _;

    value
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>()
}

#[cfg(not(any(unix, windows)))]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    value.to_string_lossy().into_owned().into_bytes()
}

#[cfg(unix)]
fn os_from_bytes(bytes: Vec<u8>) -> Result<OsString, &'static str> {
    use std::os::unix::ffi::OsStringExt as _;

    Ok(OsString::from_vec(bytes))
}

#[cfg(windows)]
fn os_from_bytes(bytes: Vec<u8>) -> Result<OsString, &'static str> {
    use std::os::windows::ffi::OsStringExt as _;

    let chunks = bytes.chunks_exact(2);
    if !chunks.remainder().is_empty() {
        return Err("cached Windows path has an odd byte count");
    }
    let wide = chunks
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    Ok(OsString::from_wide(&wide))
}

#[cfg(not(any(unix, windows)))]
fn os_from_bytes(bytes: Vec<u8>) -> Result<OsString, &'static str> {
    String::from_utf8(bytes)
        .map(OsString::from)
        .map_err(|_| "cached path is not UTF-8")
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

    use super::*;

    #[test]
    fn non_utf8_os_strings_round_trip() -> anyhow::Result<()> {
        let value = StoredOsString(OsString::from_vec(vec![b'f', 0x80, b'o']));
        let json = serde_json::to_string(&value)?;
        assert!(json.contains("unix-bytes"));
        let decoded: StoredOsString = serde_json::from_str(&json)?;
        assert_eq!(decoded.0.as_bytes(), value.0.as_bytes());
        Ok(())
    }

    #[test]
    fn unicode_os_strings_stay_readable() -> anyhow::Result<()> {
        let value = StoredOsString(OsString::from("cargo"));
        let json = serde_json::to_string(&value)?;
        assert_eq!(json, "\"cargo\"");
        Ok(())
    }
}
