use std::path::{Path, PathBuf};

use crate::scan::DiscoveryFiles;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum WrapperKind {
    Gradle,
    Maven,
}

pub(super) fn locally_usable_wrapper(
    project: &Path,
    kind: WrapperKind,
    files: &DiscoveryFiles,
) -> Option<PathBuf> {
    if kind == WrapperKind::Gradle
        && [
            "GRADLE_OPTS",
            "JAVA_OPTS",
            "JAVA_TOOL_OPTIONS",
            "_JAVA_OPTIONS",
        ]
        .into_iter()
        .any(|name| environment_contains(name, "-Dgradle.user.home"))
    {
        return None;
    }
    if kind == WrapperKind::Maven
        && ([
            "MAVEN_WRAPPER_ALWAYS_DOWNLOAD",
            "MAVEN_WRAPPER_ALWAYS_UNPACK",
        ]
        .into_iter()
        .any(environment_flag_enabled)
            || [
                "JAVA_OPTS",
                "JAVA_TOOL_OPTIONS",
                "MAVEN_OPTS",
                "_JAVA_OPTIONS",
            ]
            .into_iter()
            .any(|name| environment_contains(name, "-Dmaven.user.home")))
    {
        return None;
    }
    let (unix_name, windows_name, properties, cache_root) = match kind {
        WrapperKind::Gradle => (
            "gradlew",
            "gradlew.bat",
            "gradle/wrapper/gradle-wrapper.properties",
            gradle_cache_root()?,
        ),
        WrapperKind::Maven => (
            "mvnw",
            "mvnw.cmd",
            ".mvn/wrapper/maven-wrapper.properties",
            maven_cache_root()?,
        ),
    };
    let wrapper = [project.join(unix_name), project.join(windows_name)]
        .into_iter()
        .find(|path| usable_program(path))?;
    let properties = files.read(&project.join(properties)).ok()?;
    if !uses_default_cache_layout(&properties, kind)
        || (kind == WrapperKind::Maven
            && ["alwaysDownload", "alwaysUnpack"]
                .into_iter()
                .filter_map(|key| property(&properties, key))
                .any(|value| flag_value_enabled(std::ffi::OsStr::new(value))))
    {
        return None;
    }
    let distribution = property(&properties, "distributionUrl")?;
    let archive = distribution.rsplit('/').next()?.split('?').next()?;
    let archive_stem = archive
        .strip_suffix(".zip")
        .or_else(|| archive.strip_suffix(".tar.gz"))?;
    cached_distribution_exists(&cache_root, archive_stem).then_some(wrapper)
}

fn property<'a>(contents: &'a str, expected: &str) -> Option<&'a str> {
    contents.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == expected).then(|| value.trim())
    })
}

fn uses_default_cache_layout(contents: &str, kind: WrapperKind) -> bool {
    let expected_base = match kind {
        WrapperKind::Gradle => "GRADLE_USER_HOME",
        WrapperKind::Maven => "MAVEN_USER_HOME",
    };
    [
        ("distributionBase", expected_base),
        ("distributionPath", "wrapper/dists"),
        ("zipStoreBase", expected_base),
        ("zipStorePath", "wrapper/dists"),
    ]
    .into_iter()
    .all(|(key, expected)| property(contents, key).is_none_or(|value| value == expected))
}

fn environment_flag_enabled(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| flag_value_enabled(&value))
}

fn environment_contains(name: &str, needle: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| value.to_string_lossy().contains(needle))
}

fn flag_value_enabled(value: &std::ffi::OsStr) -> bool {
    matches!(
        value.to_string_lossy().to_ascii_lowercase().as_str(),
        "1" | "on" | "true" | "yes"
    )
}

fn gradle_cache_root() -> Option<PathBuf> {
    std::env::var_os("GRADLE_USER_HOME")
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".gradle")))
        .map(|root| root.join("wrapper/dists"))
}

fn maven_cache_root() -> Option<PathBuf> {
    std::env::var_os("MAVEN_USER_HOME")
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".m2")))
        .map(|root| root.join("wrapper/dists"))
}

fn cached_distribution_exists(root: &Path, archive_stem: &str) -> bool {
    let direct = root.join(archive_stem);
    if contains_extracted_distribution(&direct, archive_stem) {
        return true;
    }
    read_directories(root).into_iter().any(|family| {
        family
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == archive_stem)
            && contains_extracted_distribution(&family, archive_stem)
    })
}

fn contains_extracted_distribution(root: &Path, archive_stem: &str) -> bool {
    read_directories(root).into_iter().any(|hash| {
        let expected = hash.join(archive_stem);
        executable_in_distribution(&expected)
            || read_directories(&hash)
                .into_iter()
                .any(|distribution| executable_in_distribution(&distribution))
    })
}

fn executable_in_distribution(root: &Path) -> bool {
    [root.join("bin/gradle"), root.join("bin/mvn")]
        .into_iter()
        .any(|path| path.is_file())
}

fn read_directories(path: &Path) -> Vec<PathBuf> {
    let mut directories = std::fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .is_some_and(|kind| kind.is_dir())
                .then(|| entry.path())
        })
        .collect::<Vec<_>>();
    directories.sort();
    directories
}

#[cfg(unix)]
fn usable_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .ok()
        .is_some_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn usable_program(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_distribution_layout_is_recognized() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let cache = temp.path().join("wrapper/dists");
        assert!(!cached_distribution_exists(&cache, "gradle-9.1-bin"));
        std::fs::create_dir_all(cache.join("gradle-9.1-bin/hash/gradle-9.1/bin"))?;
        std::fs::write(
            cache.join("gradle-9.1-bin/hash/gradle-9.1/bin/gradle"),
            "cached",
        )?;
        assert!(cached_distribution_exists(&cache, "gradle-9.1-bin"));
        Ok(())
    }

    #[test]
    fn wrapper_install_flags_use_explicit_truthy_values() {
        for value in ["1", "on", "TRUE", "yes"] {
            assert!(flag_value_enabled(std::ffi::OsStr::new(value)), "{value}");
        }
        for value in ["", "0", "false", "off"] {
            assert!(!flag_value_enabled(std::ffi::OsStr::new(value)), "{value}");
        }
    }

    #[test]
    fn only_default_wrapper_cache_layouts_are_accepted() {
        assert!(uses_default_cache_layout(
            "distributionUrl=https\\://example.test/gradle.zip\n",
            WrapperKind::Gradle
        ));
        assert!(uses_default_cache_layout(
            "distributionBase=MAVEN_USER_HOME\ndistributionPath=wrapper/dists\n",
            WrapperKind::Maven
        ));
        assert!(!uses_default_cache_layout(
            "distributionBase=PROJECT\n",
            WrapperKind::Gradle
        ));
        assert!(!uses_default_cache_layout(
            "zipStorePath=custom/dists\n",
            WrapperKind::Maven
        ));
    }
}
