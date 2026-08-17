use std::collections::HashSet;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use directories::BaseDirs;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;
use zeroize::Zeroizing;

const STORE_VERSION: u32 = 1;
const CLIENT_KEY_BYTES: usize = 32;
const CLIENT_KEY_LENGTH: usize = 43;
const REQUIRED_PERMISSION: &str = "balances_read";

pub(crate) struct SecretClientKey(Zeroizing<String>);

impl SecretClientKey {
    pub(crate) fn from_bytes(bytes: &[u8; CLIENT_KEY_BYTES]) -> Self {
        Self(Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes)))
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn parse(value: String) -> Result<Self, ProfileError> {
        let value = Zeroizing::new(value);
        if value.len() != CLIENT_KEY_LENGTH {
            return Err(ProfileError::Invalid("invalid Client Key"));
        }

        let decoded = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(value.as_bytes())
                .map_err(|_| ProfileError::Invalid("invalid Client Key"))?,
        );
        if decoded.len() != CLIENT_KEY_BYTES
            || URL_SAFE_NO_PAD.encode(decoded.as_slice()) != value.as_str()
        {
            return Err(ProfileError::Invalid("invalid Client Key"));
        }

        Ok(Self(value))
    }
}

impl Serialize for SecretClientKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SecretClientKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(D::Error::custom)
    }
}

pub(crate) struct Profile {
    pub(crate) name: String,
    pub(crate) canonical_origin: Url,
    pub(crate) remote_user_id: String,
    pub(crate) client_key: SecretClientKey,
    pub(crate) granted_permissions: Vec<String>,
    pub(crate) allow_insecure_http: bool,
}

impl Serialize for Profile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct SerializableProfile<'a> {
            name: &'a str,
            canonical_origin: &'a str,
            remote_user_id: &'a str,
            client_key: &'a SecretClientKey,
            granted_permissions: &'a [String],
            allow_insecure_http: bool,
        }

        SerializableProfile {
            name: &self.name,
            canonical_origin: self.canonical_origin.as_str(),
            remote_user_id: &self.remote_user_id,
            client_key: &self.client_key,
            granted_permissions: &self.granted_permissions,
            allow_insecure_http: self.allow_insecure_http,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Profile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawProfile {
            name: String,
            canonical_origin: String,
            remote_user_id: String,
            client_key: SecretClientKey,
            granted_permissions: Vec<String>,
            allow_insecure_http: bool,
        }

        let raw = RawProfile::deserialize(deserializer)?;
        validate_profile_name(&raw.name).map_err(D::Error::custom)?;
        let canonical_origin =
            canonicalize_origin(&raw.canonical_origin).map_err(D::Error::custom)?;
        if canonical_origin.as_str() != raw.canonical_origin {
            return Err(D::Error::custom("profile origin is not canonical"));
        }
        if raw.remote_user_id.is_empty() {
            return Err(D::Error::custom("remote user id must not be empty"));
        }
        validate_permissions(&raw.granted_permissions).map_err(D::Error::custom)?;

        Ok(Self {
            name: raw.name,
            canonical_origin,
            remote_user_id: raw.remote_user_id,
            client_key: raw.client_key,
            granted_permissions: raw.granted_permissions,
            allow_insecure_http: raw.allow_insecure_http,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileStore {
    version: u32,
    profiles: Vec<Profile>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RenameResult {
    Renamed,
    Unknown,
    Duplicate,
}

impl ProfileStore {
    pub(crate) fn empty() -> Self {
        Self {
            version: STORE_VERSION,
            profiles: Vec::new(),
        }
    }

    pub(crate) fn from_slice(bytes: &[u8]) -> Result<Self, ProfileError> {
        let store: Self = serde_json::from_slice(bytes)
            .map_err(|_| ProfileError::Invalid("invalid profile store JSON"))?;
        store.validate()?;
        Ok(store)
    }

    pub(crate) fn load() -> Result<Self, ProfileError> {
        let path = profile_store_path()?;
        match validate_profile_directories(&path) {
            Ok(()) => {}
            Err(ProfileError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::empty());
            }
            Err(error) => return Err(error),
        }
        Self::load_from(&path)
    }

    pub(crate) fn load_from(path: &Path) -> Result<Self, ProfileError> {
        let mut file = match platform::open_secure_file(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::empty()),
            Err(error) => return Err(ProfileError::Io(error)),
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(ProfileError::Io)?;
        Self::from_slice(&bytes)
    }

    pub(crate) fn insert_saved(profile: Profile) -> Result<(), ProfileError> {
        let path = profile_store_path()?;
        ensure_profile_directory(&path)?;
        mutate_store_at(&path, |store| store.insert(profile))
    }

    pub(crate) fn remove_saved(name: &str) -> Result<bool, ProfileError> {
        let path = profile_store_path()?;
        ensure_profile_directory(&path)?;
        mutate_store_at(&path, |store| Ok(store.remove(name)))
    }

    pub(crate) fn rename_saved(old: &str, new: &str) -> Result<RenameResult, ProfileError> {
        let path = profile_store_path()?;
        ensure_profile_directory(&path)?;
        let lock =
            platform::open_lock_file(&path.with_extension("lock")).map_err(ProfileError::Io)?;
        lock.lock().map_err(ProfileError::Io)?;
        let mut store = Self::load_from(&path)?;
        let result = store.rename(old, new)?;
        if result == RenameResult::Renamed {
            store.save_to(&path)?;
        }
        Ok(result)
    }

    pub(crate) fn save_to(&self, path: &Path) -> Result<(), ProfileError> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|_| ProfileError::Invalid("profile store could not be encoded"))?;
        platform::atomic_write(path, &bytes).map_err(ProfileError::Io)
    }

    pub(crate) fn insert(&mut self, profile: Profile) -> Result<(), ProfileError> {
        validate_profile(&profile)?;
        if self
            .profiles
            .iter()
            .any(|existing| existing.name == profile.name)
        {
            return Err(ProfileError::Invalid("duplicate profile name"));
        }
        if self.profiles.iter().any(|existing| {
            existing.canonical_origin == profile.canonical_origin
                && existing.remote_user_id == profile.remote_user_id
        }) {
            return Err(ProfileError::Invalid("duplicate remote profile identity"));
        }
        self.profiles.push(profile);
        Ok(())
    }

    pub(crate) fn remove(&mut self, name: &str) -> bool {
        let original_len = self.profiles.len();
        self.profiles.retain(|profile| profile.name != name);
        self.profiles.len() != original_len
    }

    pub(crate) fn rename(&mut self, old: &str, new: &str) -> Result<RenameResult, ProfileError> {
        validate_profile_name(old)?;
        validate_profile_name(new)?;
        let Some(index) = self.profiles.iter().position(|profile| profile.name == old) else {
            return Ok(RenameResult::Unknown);
        };
        if self.profiles.iter().any(|profile| profile.name == new) {
            return Ok(RenameResult::Duplicate);
        }
        self.profiles[index].name = new.to_owned();
        Ok(RenameResult::Renamed)
    }

    pub(crate) fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.name == name)
    }

    pub(crate) fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    pub(crate) fn select(&self, name: Option<&str>) -> Result<&Profile, ProfileSelectionError> {
        if let Some(name) = name {
            return self.profile(name).ok_or_else(|| {
                ProfileSelectionError(format!(
                    "unknown profile: {name}. {}",
                    self.selection_guidance()
                ))
            });
        }
        match self.profiles.as_slice() {
            [profile] => Ok(profile),
            [] => Err(ProfileSelectionError(
                "no profiles configured; run `bitgarth pair` first".to_owned(),
            )),
            _ => Err(ProfileSelectionError(self.selection_guidance())),
        }
    }

    fn selection_guidance(&self) -> String {
        let mut names: Vec<&str> = self
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect();
        names.sort_unstable();
        if names.is_empty() {
            "no profiles are configured".to_owned()
        } else {
            format!(
                "choose one with --profile <NAME>; available profiles: {}",
                names.join(", ")
            )
        }
    }

    fn validate(&self) -> Result<(), ProfileError> {
        if self.version != STORE_VERSION {
            return Err(ProfileError::Invalid("unsupported profile store version"));
        }

        let mut names = HashSet::new();
        let mut identities = HashSet::new();
        for profile in &self.profiles {
            validate_profile(profile)?;
            if !names.insert(profile.name.as_str()) {
                return Err(ProfileError::Invalid("duplicate profile name"));
            }
            if !identities.insert((
                profile.canonical_origin.as_str(),
                profile.remote_user_id.as_str(),
            )) {
                return Err(ProfileError::Invalid("duplicate remote profile identity"));
            }
        }
        Ok(())
    }
}

fn mutate_store_at<T>(
    path: &Path,
    mutate: impl FnOnce(&mut ProfileStore) -> Result<T, ProfileError>,
) -> Result<T, ProfileError> {
    let lock = platform::open_lock_file(&path.with_extension("lock")).map_err(ProfileError::Io)?;
    lock.lock().map_err(ProfileError::Io)?;
    let mut store = ProfileStore::load_from(path)?;
    let result = mutate(&mut store)?;
    store.save_to(path)?;
    Ok(result)
}

impl Profile {
    pub(crate) fn new(
        name: String,
        canonical_origin: Url,
        remote_user_id: String,
        client_key: SecretClientKey,
        allow_insecure_http: bool,
    ) -> Result<Self, ProfileError> {
        let profile = Self {
            name,
            canonical_origin,
            remote_user_id,
            client_key,
            granted_permissions: vec![REQUIRED_PERMISSION.to_owned()],
            allow_insecure_http,
        };
        validate_profile(&profile)?;
        Ok(profile)
    }

    pub(crate) fn canonical_origin(&self) -> &Url {
        &self.canonical_origin
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn client_key(&self) -> &SecretClientKey {
        &self.client_key
    }

    pub(crate) fn allows_insecure_http(&self) -> bool {
        self.allow_insecure_http
    }
}

pub(crate) fn validate_profile_name(name: &str) -> Result<(), ProfileError> {
    let scalar_count = name.chars().count();
    if scalar_count == 0 || scalar_count > 64 || name.len() > 256 {
        return Err(ProfileError::Invalid(
            "profile name must contain 1 to 64 characters",
        ));
    }
    if name.trim() != name || name.chars().any(char::is_control) {
        return Err(ProfileError::Invalid(
            "profile name contains forbidden whitespace or control characters",
        ));
    }
    Ok(())
}

pub(crate) fn canonicalize_origin(input: &str) -> Result<Url, ProfileError> {
    let Some((scheme, authority)) = input.split_once("://") else {
        return Err(ProfileError::Invalid("invalid server origin"));
    };
    if scheme.is_empty() || authority.is_empty() || authority.starts_with('/') {
        return Err(ProfileError::Invalid("invalid server origin"));
    }

    let mut origin =
        Url::parse(input).map_err(|_| ProfileError::Invalid("invalid server origin"))?;
    if !matches!(origin.scheme(), "http" | "https")
        || origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
    {
        return Err(ProfileError::Invalid("invalid server origin"));
    }

    origin.set_path("/");
    origin.set_query(None);
    origin.set_fragment(None);

    let default_port = match origin.scheme() {
        "http" => 80,
        "https" => 443,
        _ => return Err(ProfileError::Invalid("invalid server origin")),
    };
    if origin.port() == Some(default_port) {
        origin
            .set_port(None)
            .map_err(|()| ProfileError::Invalid("invalid server origin"))?;
    }
    Ok(origin)
}

fn validate_profile(profile: &Profile) -> Result<(), ProfileError> {
    validate_profile_name(&profile.name)?;
    if profile.remote_user_id.is_empty() {
        return Err(ProfileError::Invalid("remote user id must not be empty"));
    }
    if canonicalize_origin(profile.canonical_origin.as_str())? != profile.canonical_origin {
        return Err(ProfileError::Invalid("profile origin is not canonical"));
    }
    if !matches!(
        (
            profile.canonical_origin.scheme(),
            profile.allow_insecure_http
        ),
        ("https", false) | ("http", true)
    ) {
        return Err(ProfileError::Invalid(
            "profile transport consent does not match its origin",
        ));
    }
    validate_permissions(&profile.granted_permissions)
}

fn validate_permissions(permissions: &[String]) -> Result<(), ProfileError> {
    if permissions.len() != 1 || permissions[0] != REQUIRED_PERMISSION {
        return Err(ProfileError::Invalid("unsupported profile permissions"));
    }
    Ok(())
}

fn profile_store_path() -> Result<PathBuf, ProfileError> {
    let base_dirs = BaseDirs::new().ok_or(ProfileError::Invalid(
        "local config directory is unavailable",
    ))?;
    Ok(base_dirs
        .config_local_dir()
        .join("BitGarth")
        .join("cli")
        .join("profiles.json"))
}

fn ensure_profile_directory(path: &Path) -> Result<(), ProfileError> {
    let cli_dir = path
        .parent()
        .ok_or(ProfileError::Invalid("invalid profile store path"))?;
    let bitgarth_dir = cli_dir
        .parent()
        .ok_or(ProfileError::Invalid("invalid profile store path"))?;
    platform::ensure_secure_directory(bitgarth_dir).map_err(ProfileError::Io)?;
    platform::ensure_secure_directory(cli_dir).map_err(ProfileError::Io)
}

fn validate_profile_directories(path: &Path) -> Result<(), ProfileError> {
    let cli_dir = path
        .parent()
        .ok_or(ProfileError::Invalid("invalid profile store path"))?;
    let bitgarth_dir = cli_dir
        .parent()
        .ok_or(ProfileError::Invalid("invalid profile store path"))?;
    platform::validate_secure_directory(bitgarth_dir).map_err(ProfileError::Io)?;
    platform::validate_secure_directory(cli_dir).map_err(ProfileError::Io)
}

#[derive(Debug)]
pub(crate) enum ProfileError {
    Io(io::Error),
    Invalid(&'static str),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "profile storage error: {error}"),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ProfileError {}

pub(crate) struct ProfileSelectionError(String);

impl fmt::Display for ProfileSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for ProfileSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for ProfileSelectionError {}

#[cfg(unix)]
mod platform {
    use std::fs::{self, DirBuilder, File, OpenOptions};
    use std::io::{self, Write};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};

    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    const DIRECTORY_MODE: u32 = 0o700;
    const FILE_MODE: u32 = 0o600;

    pub(super) fn ensure_secure_directory(path: &Path) -> io::Result<()> {
        match DirBuilder::new().mode(DIRECTORY_MODE).create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }

        validate_secure_directory(path)
    }

    pub(super) fn validate_secure_directory(path: &Path) -> io::Result<()> {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)?;
        validate_directory_metadata(&directory.metadata()?)
    }

    pub(super) fn open_secure_file(path: &Path) -> io::Result<File> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "profile path has no parent")
        })?;
        validate_secure_directory(parent)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != FILE_MODE
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "profile store must be an owner-only regular file",
            ));
        }
        Ok(file)
    }

    pub(super) fn open_lock_file(path: &Path) -> io::Result<File> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "profile path has no parent")
        })?;
        validate_secure_directory(parent)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(FILE_MODE)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != FILE_MODE
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "profile lock must be an owner-only regular file",
            ));
        }
        Ok(file)
    }

    pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "profile path has no parent")
        })?;
        let parent_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(parent)?;
        validate_directory_metadata(&parent_file.metadata()?)?;

        match open_secure_file(path) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let (temp_path, mut temp_file) = create_unique_temp(path)?;
        let mut cleanup = TempCleanup(Some(temp_path.clone()));
        temp_file.write_all(bytes)?;
        temp_file.write_all(b"\n")?;
        temp_file.sync_all()?;
        fs::rename(&temp_path, path)?;
        cleanup.0 = None;
        parent_file.sync_all()
    }

    fn create_unique_temp(path: &Path) -> io::Result<(PathBuf, File)> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid profile file name")
            })?;
        for _ in 0..32 {
            let mut random = [0_u8; 12];
            getrandom::fill(&mut random)
                .map_err(|_| io::Error::other("OS randomness unavailable"))?;
            let candidate =
                path.with_file_name(format!(".{file_name}.{}", URL_SAFE_NO_PAD.encode(random)));
            let opened = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(FILE_MODE)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&candidate);
            match opened {
                Ok(file) => return Ok((candidate, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create unique profile store sibling",
        ))
    }

    fn validate_directory_metadata(metadata: &fs::Metadata) -> io::Result<()> {
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != DIRECTORY_MODE
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "profile directories must be owner-only directories",
            ));
        }
        Ok(())
    }

    struct TempCleanup(Option<PathBuf>);

    impl Drop for TempCleanup {
        fn drop(&mut self) {
            if let Some(path) = self.0.take() {
                let _ = fs::remove_file(path);
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Write};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};

    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, ReplaceFileW,
    };

    pub(super) fn ensure_secure_directory(path: &Path) -> io::Result<()> {
        fs::create_dir(path).or_else(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })?;
        reject_reparse(path, true)?;
        protect_current_user(path)
    }

    pub(super) fn validate_secure_directory(path: &Path) -> io::Result<()> {
        reject_reparse(path, true)
    }

    pub(super) fn open_secure_file(path: &Path) -> io::Result<File> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "profile path has no parent")
        })?;
        validate_secure_directory(parent)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "profile store must be a non-reparse regular file",
            ));
        }
        Ok(file)
    }

    pub(super) fn open_lock_file(path: &Path) -> io::Result<File> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "profile path has no parent")
        })?;
        validate_secure_directory(parent)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "profile lock must be a non-reparse regular file",
            ));
        }
        protect_current_user(path)?;
        Ok(file)
    }

    pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "profile path has no parent")
        })?;
        reject_reparse(parent, true)?;
        match open_secure_file(path) {
            Ok(_) => protect_current_user(path)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let (temp_path, mut temp_file) = create_unique_temp(path)?;
        let mut cleanup = TempCleanup(Some(temp_path.clone()));
        protect_current_user(&temp_path)?;
        temp_file.write_all(bytes)?;
        temp_file.write_all(b"\n")?;
        temp_file.sync_all()?;
        drop(temp_file);

        if path.exists() {
            let replaced = unsafe {
                ReplaceFileW(
                    wide(path).as_ptr(),
                    wide(&temp_path).as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            if replaced == 0 {
                return Err(io::Error::last_os_error());
            }
        } else {
            fs::rename(&temp_path, path)?;
        }
        cleanup.0 = None;
        Ok(())
    }

    fn create_unique_temp(path: &Path) -> io::Result<(PathBuf, File)> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid profile file name")
            })?;
        for _ in 0..32 {
            let mut random = [0_u8; 12];
            getrandom::fill(&mut random)
                .map_err(|_| io::Error::other("OS randomness unavailable"))?;
            let candidate =
                path.with_file_name(format!(".{file_name}.{}", URL_SAFE_NO_PAD.encode(random)));
            let opened = OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&candidate);
            match opened {
                Ok(file) => return Ok((candidate, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create unique profile store sibling",
        ))
    }

    fn reject_reparse(path: &Path, directory: bool) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        let correct_kind = if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        };
        if !correct_kind || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "profile path must not be a reparse point",
            ));
        }
        Ok(())
    }

    fn protect_current_user(path: &Path) -> io::Result<()> {
        // Windows creates these objects under the caller's token. A protected,
        // current-user-only DACL is applied here before any secret bytes are written.
        super::windows_acl::protect_current_user(path)
    }

    fn wide(path: &Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt as _;
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    struct TempCleanup(Option<PathBuf>);

    impl Drop for TempCleanup {
        fn drop(&mut self) {
            if let Some(path) = self.0.take() {
                let _ = fs::remove_file(path);
            }
        }
    }
}

#[cfg(windows)]
mod windows_acl {
    use std::ffi::c_void;
    use std::io;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAceEx, CONTAINER_INHERIT_ACE,
        DACL_SECURITY_INFORMATION, GetLengthSid, GetTokenInformation, InitializeAcl,
        OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    pub(super) fn protect_current_user(path: &Path) -> io::Result<()> {
        let token = current_process_token()?;
        let result = apply_current_user_acl(path, token);
        unsafe {
            CloseHandle(token);
        }
        result
    }

    fn current_process_token() -> io::Result<HANDLE> {
        let mut token = std::ptr::null_mut();
        let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
        if opened == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(token)
        }
    }

    fn apply_current_user_acl(path: &Path, token: HANDLE) -> io::Result<()> {
        let mut required_bytes = 0_u32;
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut required_bytes,
            );
        }
        if required_bytes == 0 {
            return Err(io::Error::last_os_error());
        }

        let mut token_buffer = aligned_buffer(required_bytes as usize);
        let loaded = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                token_buffer.as_mut_ptr().cast::<c_void>(),
                required_bytes,
                &mut required_bytes,
            )
        };
        if loaded == 0 {
            return Err(io::Error::last_os_error());
        }
        let token_user = unsafe { &*token_buffer.as_ptr().cast::<TOKEN_USER>() };
        let sid_length = unsafe { GetLengthSid(token_user.User.Sid) } as usize;
        if sid_length == 0 {
            return Err(io::Error::last_os_error());
        }

        let acl_bytes =
            size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>() + sid_length;
        let mut acl_buffer = aligned_buffer(acl_bytes);
        let acl = acl_buffer.as_mut_ptr().cast::<ACL>();
        if unsafe { InitializeAcl(acl, acl_bytes as u32, ACL_REVISION) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let inheritance = if path.is_dir() {
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
        } else {
            0
        };
        if unsafe {
            AddAccessAllowedAceEx(
                acl,
                ACL_REVISION,
                inheritance,
                FILE_ALL_ACCESS,
                token_user.User.Sid,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }

        let mut path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let status = unsafe {
            SetNamedSecurityInfoW(
                path_wide.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null(),
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status as i32))
        }
    }

    fn aligned_buffer(bytes: usize) -> Vec<usize> {
        vec![0; bytes.div_ceil(size_of::<usize>())]
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::{
        Profile, ProfileStore, RenameResult, SecretClientKey, canonicalize_origin,
        validate_profile_name,
    };
    use base64::Engine as _;

    const KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    #[test]
    fn profile_names_follow_the_exact_contract() {
        assert!(validate_profile_name("personal").is_ok());
        assert!(validate_profile_name(".").is_ok());
        assert!(validate_profile_name("..").is_ok());
        assert!(validate_profile_name("a/b\\c").is_ok());
        assert!(validate_profile_name("").is_err());
        assert!(validate_profile_name(" personal").is_err());
        assert!(validate_profile_name("personal ").is_err());
        assert!(validate_profile_name("line\nbreak").is_err());
        assert!(validate_profile_name(&"x".repeat(65)).is_err());
        assert!(validate_profile_name(&"🦀".repeat(64)).is_ok());
        assert!(validate_profile_name(&format!("{}a", "🦀".repeat(64))).is_err());
    }

    #[test]
    fn rejects_invalid_store_json_without_relaxing_required_fields() {
        assert!(ProfileStore::from_slice(b"not json").is_err());
        assert!(ProfileStore::from_slice(br#"{"version":2,"profiles":[]}"#).is_err());
        assert!(ProfileStore::from_slice(br#"{"version":1,"profiles":[{}]}"#).is_err());
        assert!(ProfileStore::from_slice(br#"{"version":1,"version":1,"profiles":[]}"#).is_err());
    }

    #[test]
    fn rejects_noncanonical_keys_and_origins() {
        let padded_key = format!("{KEY}=");
        assert!(
            ProfileStore::from_slice(
                profile_json("a", "https://example.com/", "user", &padded_key).as_bytes()
            )
            .is_err()
        );
        assert!(
            ProfileStore::from_slice(
                profile_json("a", "https://EXAMPLE.com/", "user", KEY).as_bytes()
            )
            .is_err()
        );
        assert!(
            ProfileStore::from_slice(
                profile_json("a", "https://example.com:443/", "user", KEY).as_bytes()
            )
            .is_err()
        );
        assert_eq!(
            canonicalize_origin("https://EXAMPLE.com:443/")
                .map(|origin| origin.to_string())
                .ok(),
            Some("https://example.com/".to_owned())
        );
    }

    #[test]
    fn browser_urls_reduce_to_canonical_origins() {
        let cases = [
            (
                "http://127.0.0.1:8080/reports/holdings?start=2026-01-01&end=2026-08-07",
                "http://127.0.0.1:8080/",
            ),
            (
                "https://EXAMPLE.com:443/settings/clients?tab=active#pairing",
                "https://example.com/",
            ),
            ("http://example.com:80/a/b", "http://example.com/"),
            ("https://example.com:8443/a/b", "https://example.com:8443/"),
        ];

        for (input, expected) in cases {
            assert_eq!(
                canonicalize_origin(input)
                    .ok()
                    .map(|origin| origin.to_string()),
                Some(expected.to_owned())
            );
        }

        assert!(canonicalize_origin("ftp://example.com/path").is_err());
        assert!(canonicalize_origin("https:///missing-host").is_err());
        assert!(canonicalize_origin("https:example.com").is_err());
        assert!(canonicalize_origin("https:/example.com").is_err());
        assert!(canonicalize_origin("https://user@example.com/path").is_err());
        assert!(canonicalize_origin("https://user:secret@example.com/path").is_err());
    }

    #[test]
    fn duplicate_names_and_remote_identities_are_rejected_without_replacement() {
        let mut store = ProfileStore::empty();
        assert!(
            store
                .insert(profile("one", "https://example.com/", "user-1"))
                .is_ok()
        );
        assert!(
            store
                .insert(profile("one", "https://other.example/", "user-2"))
                .is_err()
        );
        assert!(
            store
                .insert(profile("two", "https://example.com/", "user-1"))
                .is_err()
        );
        assert_eq!(store.profiles.len(), 1);
        assert_eq!(
            store
                .profile("one")
                .map(|item| item.remote_user_id.as_str()),
            Some("user-1")
        );
    }

    #[test]
    fn distinct_origins_and_users_remain_distinct() {
        let mut store = ProfileStore::empty();
        assert!(
            store
                .insert(profile("one", "https://example.com/", "user-1"))
                .is_ok()
        );
        assert!(
            store
                .insert(profile("two", "https://example.com/", "user-2"))
                .is_ok()
        );
        assert!(
            store
                .insert(profile("three", "https://other.example/", "user-1"))
                .is_ok()
        );
        assert_eq!(store.profiles.len(), 3);
    }

    #[test]
    fn remove_targets_only_the_named_profile() {
        let mut store = ProfileStore::empty();
        assert!(
            store
                .insert(profile("one", "https://example.com/", "user-1"))
                .is_ok()
        );
        assert!(
            store
                .insert(profile("two", "https://example.com/", "user-2"))
                .is_ok()
        );
        assert!(store.remove("one"));
        assert!(!store.remove("missing"));
        assert!(store.profile("one").is_none());
        assert!(store.profile("two").is_some());
    }

    #[test]
    fn rename_changes_only_the_local_profile_name() {
        let mut store = ProfileStore::empty();
        assert!(
            store
                .insert(profile("personal", "https://example.com/", "user-1"))
                .is_ok()
        );
        let before_origin = store
            .profile("personal")
            .map(|profile| profile.canonical_origin.as_str().to_owned());
        let before_user = store
            .profile("personal")
            .map(|profile| profile.remote_user_id.clone());
        let before_key = store
            .profile("personal")
            .map(|profile| profile.client_key.as_str().to_owned());
        let before_permissions = store
            .profile("personal")
            .map(|profile| profile.granted_permissions.clone());
        let before_http_consent = store
            .profile("personal")
            .map(|profile| profile.allow_insecure_http);

        assert_eq!(
            store.rename("personal", "primary").ok(),
            Some(RenameResult::Renamed)
        );
        assert!(store.profile("personal").is_none());
        let renamed = store.profile("primary");
        assert_eq!(
            renamed.map(|profile| profile.canonical_origin.as_str().to_owned()),
            before_origin
        );
        assert_eq!(
            renamed.map(|profile| profile.remote_user_id.clone()),
            before_user
        );
        assert_eq!(
            renamed.map(|profile| profile.client_key.as_str().to_owned()),
            before_key
        );
        assert_eq!(
            renamed.map(|profile| profile.granted_permissions.clone()),
            before_permissions
        );
        assert_eq!(
            renamed.map(|profile| profile.allow_insecure_http),
            before_http_consent
        );
    }

    #[test]
    fn rename_reports_unknown_duplicate_and_invalid_names_without_logical_changes() {
        let mut store = ProfileStore::empty();
        assert!(
            store
                .insert(profile("personal", "https://example.com/", "user-1"))
                .is_ok()
        );
        assert!(
            store
                .insert(profile("work", "https://work.example/", "user-2"))
                .is_ok()
        );

        assert_eq!(
            store.rename("missing", "primary").ok(),
            Some(RenameResult::Unknown)
        );
        assert_eq!(
            store.rename("personal", "work").ok(),
            Some(RenameResult::Duplicate)
        );
        assert_eq!(
            store.rename("personal", "personal").ok(),
            Some(RenameResult::Duplicate)
        );
        assert!(store.rename(" bad", "primary").is_err());
        assert!(store.rename("personal", " bad").is_err());
        assert!(store.profile("personal").is_some());
        assert!(store.profile("work").is_some());
        assert_eq!(store.profiles.len(), 2);
    }

    #[test]
    fn selection_infers_one_and_guides_zero_unknown_or_multiple() {
        let empty = ProfileStore::empty();
        assert!(empty.select(None).is_err());

        let mut one = ProfileStore::empty();
        assert!(
            one.insert(profile("business", "https://example.com/", "user-1"))
                .is_ok()
        );
        assert_eq!(
            one.select(None).ok().map(|profile| profile.name.as_str()),
            Some("business")
        );
        assert!(one.select(Some("missing")).is_err());

        assert!(
            one.insert(profile("alpha", "https://other.example/", "user-2"))
                .is_ok()
        );
        let error = one.select(None).err().map(|error| error.to_string());
        assert_eq!(
            error.as_deref(),
            Some("choose one with --profile <NAME>; available profiles: alpha, business")
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_validation_preserves_original_file_bytes() {
        use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};

        let directory = test_directory();
        let created = std::fs::DirBuilder::new().mode(0o700).create(&directory);
        assert!(created.is_ok());
        let path = directory.join("profiles.json");
        let original = b"malformed and precious";
        let opened = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path);
        assert!(opened.is_ok());
        if let Ok(mut file) = opened {
            use std::io::Write as _;
            assert!(file.write_all(original).is_ok());
        }

        assert!(ProfileStore::load_from(&path).is_err());
        assert_eq!(fs::read(&path).ok().as_deref(), Some(original.as_slice()));
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_store_round_trip_uses_owner_only_file() {
        use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};

        let directory = test_directory();
        let created = std::fs::DirBuilder::new().mode(0o700).create(&directory);
        assert!(created.is_ok());
        let path = directory.join("profiles.json");
        let mut store = ProfileStore::empty();
        assert!(
            store
                .insert(profile("one", "https://example.com/", "user-1"))
                .is_ok()
        );

        assert!(store.save_to(&path).is_ok());
        let metadata = fs::metadata(&path);
        assert!(metadata.is_ok());
        if let Ok(metadata) = metadata {
            assert_eq!(metadata.mode() & 0o777, 0o600);
        }
        let loaded = ProfileStore::load_from(&path);
        assert!(loaded.is_ok());
        if let Ok(loaded) = loaded {
            assert_eq!(loaded.profiles.len(), 1);
            assert!(loaded.profile("one").is_some());
        }
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn concurrent_store_mutations_reload_after_lock_acquisition() {
        let directory = test_directory();
        assert!(super::platform::ensure_secure_directory(&directory).is_ok());
        let path = directory.join("profiles.json");
        let (first_entered_tx, first_entered_rx) = mpsc::channel();
        let (first_release_tx, first_release_rx) = mpsc::channel();
        let first_path = path.clone();
        let first = thread::spawn(move || {
            super::mutate_store_at(&first_path, |store| {
                assert!(first_entered_tx.send(()).is_ok());
                assert!(first_release_rx.recv().is_ok());
                store.insert(profile("one", "https://one.example/", "user-1"))
            })
        });
        assert!(
            first_entered_rx
                .recv_timeout(Duration::from_secs(1))
                .is_ok()
        );

        let (second_attempting_tx, second_attempting_rx) = mpsc::channel();
        let (second_entered_tx, second_entered_rx) = mpsc::channel();
        let second_path = path.clone();
        let second = thread::spawn(move || {
            assert!(second_attempting_tx.send(()).is_ok());
            super::mutate_store_at(&second_path, |store| {
                assert!(second_entered_tx.send(()).is_ok());
                store.insert(profile("two", "https://two.example/", "user-2"))
            })
        });
        assert!(
            second_attempting_rx
                .recv_timeout(Duration::from_secs(1))
                .is_ok()
        );
        let second_entered_early = second_entered_rx
            .recv_timeout(Duration::from_millis(50))
            .is_ok();

        assert!(first_release_tx.send(()).is_ok());
        let first_result = first.join();
        let second_result = second.join();
        assert!(matches!(first_result, Ok(Ok(()))));
        assert!(matches!(second_result, Ok(Ok(()))));

        let loaded = ProfileStore::load_from(&path);
        let has_one = loaded
            .as_ref()
            .ok()
            .is_some_and(|store| store.profile("one").is_some());
        let has_two = loaded
            .as_ref()
            .ok()
            .is_some_and(|store| store.profile("two").is_some());
        let _ = fs::remove_dir_all(directory);
        assert!(
            !second_entered_early,
            "a second profile mutation entered while the first still held the store lock"
        );
        assert!(loaded.is_ok());
        assert!(has_one);
        assert!(has_two);
    }

    #[cfg(unix)]
    #[test]
    fn insecure_existing_file_is_rejected_without_replacement() {
        use std::io::Write as _;
        use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};

        let directory = test_directory();
        let created = std::fs::DirBuilder::new().mode(0o700).create(&directory);
        assert!(created.is_ok());
        let path = directory.join("profiles.json");
        let original = b"do not replace";
        let opened = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&path);
        assert!(opened.is_ok());
        if let Ok(mut file) = opened {
            assert!(file.write_all(original).is_ok());
        }

        assert!(ProfileStore::empty().save_to(&path).is_err());
        assert_eq!(fs::read(&path).ok().as_deref(), Some(original.as_slice()));
        let _ = fs::remove_dir_all(directory);
    }

    fn profile(name: &str, origin: &str, remote_user_id: &str) -> Profile {
        let canonical_origin = canonicalize_origin(origin);
        assert!(canonical_origin.is_ok());
        Profile {
            name: name.to_owned(),
            canonical_origin: match canonical_origin {
                Ok(origin) => origin,
                Err(_) => std::process::abort(),
            },
            remote_user_id: remote_user_id.to_owned(),
            client_key: SecretClientKey::from_bytes(&[0; 32]),
            granted_permissions: vec!["balances_read".to_owned()],
            allow_insecure_http: false,
        }
    }

    fn profile_json(name: &str, origin: &str, user_id: &str, key: &str) -> String {
        format!(
            r#"{{"version":1,"profiles":[{{"name":"{name}","canonical_origin":"{origin}","remote_user_id":"{user_id}","client_key":"{key}","granted_permissions":["balances_read"],"allow_insecure_http":false}}]}}"#
        )
    }

    fn test_directory() -> PathBuf {
        let mut random = [0_u8; 12];
        assert!(getrandom::fill(&mut random).is_ok());
        let suffix = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
        std::env::temp_dir().join(format!("bitgarth-cli-profile-test-{suffix}"))
    }
}
