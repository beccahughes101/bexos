#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub build: u32,
    pub prerelease: String,
}

impl SemVer {
    pub fn is_legacy(&self) -> bool {
        self.major == 0
            && self.minor == 0
            && self.patch == 0
            && self.build == 0
            && self.prerelease.is_empty()
    }

    pub fn label(&self) -> String {
        if self.is_legacy() {
            return "0.0.0".into();
        }
        let mut label = format!("{}.{}.{}", self.major, self.minor, self.patch);
        if self.build != 0 {
            label.push_str("-b");
            label.push_str(&self.build.to_string());
        }
        if !self.prerelease.is_empty() {
            label.push('-');
            label.push_str(&self.prerelease);
        }
        label
    }

    pub fn matches_prefix(&self, requirement: &str) -> bool {
        let requirement = normalize_version_requirement(requirement);
        if requirement.is_empty() {
            return true;
        }
        self.label().starts_with(requirement)
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.major,
            self.minor,
            self.patch,
            self.build,
            &self.prerelease,
        )
            .cmp(&(
                other.major,
                other.minor,
                other.patch,
                other.build,
                &other.prerelease,
            ))
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MultiVersionPolicy {
    #[default]
    SingleActiveOnly,
    ParallelExecution,
    SharedStorageMulti,
    Unspecified,
}

impl MultiVersionPolicy {
    pub fn from_proto(value: u64) -> Self {
        match value {
            0 => Self::SingleActiveOnly,
            1 => Self::ParallelExecution,
            2 => Self::SharedStorageMulti,
            _ => Self::Unspecified,
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::SingleActiveOnly => 1,
            Self::ParallelExecution => 2,
            Self::SharedStorageMulti => 3,
            Self::Unspecified => 0,
        }
    }

    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Unspecified),
            1 => Some(Self::SingleActiveOnly),
            2 => Some(Self::ParallelExecution),
            3 => Some(Self::SharedStorageMulti),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HealthCheckStatus {
    #[default]
    Healthy,
    Probation,
    CrashLoop,
}

impl HealthCheckStatus {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Healthy => 1,
            Self::Probation => 2,
            Self::CrashLoop => 3,
        }
    }

    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Healthy),
            2 => Some(Self::Probation),
            3 => Some(Self::CrashLoop),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageSelector {
    pub package: String,
    pub version_requirement: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VersionMatchError {
    InvalidSelector,
    NotFound,
    Ambiguous,
}

pub fn parse_package_selector(
    package: &str,
    version_requirement: Option<&str>,
) -> Result<PackageSelector, VersionMatchError> {
    let package = package.trim();
    if package.is_empty() {
        return Err(VersionMatchError::InvalidSelector);
    }
    let mut base = package;
    let mut version = version_requirement
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    if version.is_none() {
        if let Some((candidate_base, candidate_version)) = split_versioned_package(package) {
            base = candidate_base;
            version = Some(candidate_version.to_string());
        }
    }
    Ok(PackageSelector {
        package: base.to_string(),
        version_requirement: version,
    })
}

pub fn split_versioned_package(package: &str) -> Option<(&str, &str)> {
    let (base, suffix) = package.rsplit_once(':')?;
    let normalized = normalize_version_requirement(suffix);
    if base.is_empty() || normalized.is_empty() || !starts_like_version(suffix) {
        return None;
    }
    Some((base, suffix))
}

pub fn versioned_package_key(package_id: &str, version: &SemVer) -> String {
    if version.is_legacy() {
        package_id.to_string()
    } else {
        format!("{}:{}", package_id, version.label())
    }
}

pub fn normalize_version_requirement(requirement: &str) -> &str {
    requirement.trim().trim_start_matches('v')
}

fn starts_like_version(value: &str) -> bool {
    let value = value.trim();
    value.starts_with('v') || value.bytes().next().is_some_and(|b| b.is_ascii_digit())
}

pub fn select_unique_match<'a, T, F>(
    items: &'a [T],
    selector: &PackageSelector,
    mut package_and_version: F,
) -> Result<&'a T, VersionMatchError>
where
    F: FnMut(&'a T) -> (&'a str, &'a SemVer),
{
    let requirement = selector
        .version_requirement
        .as_deref()
        .map(normalize_version_requirement);
    let mut matches = Vec::new();
    for item in items {
        let (package, version) = package_and_version(item);
        if !package.starts_with(selector.package.as_str()) {
            continue;
        }
        if requirement.is_some_and(|req| !version.matches_prefix(req)) {
            continue;
        }
        matches.push(item);
    }
    if matches.is_empty() {
        return Err(VersionMatchError::NotFound);
    }
    matches.sort_by(|left, right| {
        let (left_package, left_version) = package_and_version(left);
        let (right_package, right_version) = package_and_version(right);
        left_package
            .cmp(right_package)
            .then_with(|| left_version.cmp(right_version))
            .reverse()
    });
    if matches.len() > 1 {
        let (first_package, first_version) = package_and_version(matches[0]);
        let (second_package, second_version) = package_and_version(matches[1]);
        if first_package != second_package || first_version != second_version {
            return Err(VersionMatchError::Ambiguous);
        }
    }
    Ok(matches[0])
}
