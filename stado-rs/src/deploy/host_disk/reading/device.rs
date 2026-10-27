//! One block device as the host's `lsblk -b -P` named it, its type and
//! filesystem signature parsed once into the cases the reading decides on.

/// One block device as the host's `lsblk -b -P` named it: a disk, a
/// partition or a mapper volume, with the filesystem on it and where it is
/// mounted, when it is. `mountpoint` empty on a `disk` or `part` with no
/// children is the fact this record exists for: storage the host has and
/// the fleet cannot write to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockDevice {
    pub name: String,
    /// The size in bytes, absent when lsblk printed none or not a count.
    pub size_bytes: Option<i64>,
    /// Why `size_bytes` is absent when lsblk printed a size that is not a
    /// byte count.
    pub size_unread: Option<String>,
    /// `TYPE` as lsblk printed it.
    pub kind: String,
    /// `FSTYPE` as lsblk printed it.
    pub fstype: String,
    pub mountpoint: String,
    pub uuid: String,
    pub model: String,
    /// `kind`, parsed once where the row is read.
    pub device_type: DeviceType,
    /// `fstype`, parsed once where the row is read.
    pub signature: Signature,
}

/// The `lsblk` device types this reading decides on; every other type
/// (`lvm`, `crypt`, `loop`, `rom`, `raid…`) is a layer above a disk or not
/// storage of its own, as is a row that names no type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    Disk,
    Part,
    #[default]
    #[serde(other)]
    Other,
}

/// The `lsblk` filesystem signatures that are in use without a mountpoint;
/// every other signature, and none, is `Other`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
pub enum Signature {
    /// An LVM physical volume.
    #[serde(rename = "LVM2_member")]
    LvmMember,
    /// A LUKS container.
    #[serde(rename = "crypto_LUKS")]
    Luks,
    /// Swap space.
    #[serde(rename = "swap")]
    Swap,
    #[default]
    #[serde(other)]
    Other,
}

/// The variant `word` names. Every word names one: each enum's `Other`
/// carries `#[serde(other)]`, so the error arm is only the type's own
/// catch-all, never a word read as something it is not.
fn parsed<T: serde::de::DeserializeOwned + Default>(word: &str) -> T {
    use serde::de::IntoDeserializer;
    let deserializer: serde::de::value::StrDeserializer<'_, serde::de::value::Error> =
        word.into_deserializer();
    match T::deserialize(deserializer) {
        Ok(variant) => variant,
        Err(_unlisted_word) => T::default(),
    }
}

impl DeviceType {
    pub fn of(word: &str) -> Self {
        parsed(word)
    }
}

impl Signature {
    pub fn of(word: &str) -> Self {
        parsed(word)
    }
}

impl BlockDevice {
    /// A whole disk or partition holding no mounted filesystem, no partition
    /// of its own that is listed, and no filesystem that is in use without
    /// a mountpoint (an LVM physical volume, a LUKS container, swap):
    /// attached, and unused by anything the kernel mounts. A device whose
    /// size lsblk did not state as a positive count is not claimed as storage.
    pub fn unmounted_among(&self, all: &[BlockDevice]) -> bool {
        if self.device_type == DeviceType::Other
            || !self.mountpoint.is_empty()
            || !self.size_bytes.is_some_and(i64::is_positive)
            || self.signature != Signature::Other
        {
            return false;
        }
        !all.iter().any(|other| {
            other.name != self.name
                && other.name.starts_with(&self.name)
                && (other.device_type == DeviceType::Part || !other.mountpoint.is_empty())
        })
    }
}
