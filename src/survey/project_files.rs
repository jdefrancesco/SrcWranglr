//! What a project's own files say about it: how it is built, what
//! documentation it has, its license, and its CI.

/// Build systems by the files that mark them: exact names, then extensions.
const BUILD_SYSTEMS: &[(&str, &[&str], &[&str])] = &[
    ("CMake", &["CMakeLists.txt"], &[]),
    ("Make", &["Makefile", "makefile", "GNUmakefile"], &[]),
    (
        "Autotools",
        &["configure.ac", "configure.in", "Makefile.am"],
        &[],
    ),
    ("Meson", &["meson.build"], &[]),
    (
        "Bazel",
        &[
            "BUILD.bazel",
            "WORKSPACE",
            "WORKSPACE.bazel",
            "MODULE.bazel",
        ],
        &["bzl"],
    ),
    ("SCons", &["SConstruct"], &[]),
    ("Ninja", &["build.ninja"], &[]),
    ("Visual Studio / MSBuild", &[], &["sln", "vcxproj"]),
    ("Xcode", &[], &["xcodeproj"]),
    ("Premake", &["premake5.lua", "premake4.lua"], &[]),
    ("xmake", &["xmake.lua"], &[]),
    ("QMake", &[], &["pro"]),
    ("Kbuild / Kconfig", &["Kbuild", "Kconfig"], &[]),
    ("Gradle", &["build.gradle", "build.gradle.kts"], &[]),
    ("Maven", &["pom.xml"], &[]),
    ("Cargo", &["Cargo.toml"], &[]),
    ("npm", &["package.json"], &[]),
    ("Python packaging", &["setup.py", "pyproject.toml"], &[]),
    ("Conan", &["conanfile.txt", "conanfile.py"], &[]),
    ("vcpkg", &["vcpkg.json"], &[]),
    ("Nix", &["flake.nix", "default.nix"], &[]),
    ("Docker", &["Dockerfile"], &[]),
];

/// Top-level documents worth reading first, by name prefix (uppercase).
const DOC_PREFIXES: &[&str] = &[
    "README",
    "LICENSE",
    "LICENCE",
    "COPYING",
    "CONTRIBUTING",
    "SECURITY",
    "CHANGELOG",
    "CHANGES",
    "NEWS",
    "AUTHORS",
    "INSTALL",
    "BUILDING",
    "HACKING",
    "NOTICE",
];
const DOC_DIRS: &[&str] = &["doc", "docs", "documentation", "man"];
/// Top-level names (files or folders) that configure CI.
const CI_MARKERS: &[(&str, &str)] = &[
    (".github", "GitHub Actions"),
    (".gitlab-ci.yml", "GitLab CI"),
    (".travis.yml", "Travis CI"),
    (".circleci", "CircleCI"),
    ("azure-pipelines.yml", "Azure Pipelines"),
    ("appveyor.yml", "AppVeyor"),
    (".appveyor.yml", "AppVeyor"),
    ("Jenkinsfile", "Jenkins"),
    (".cirrus.yml", "Cirrus CI"),
    (".drone.yml", "Drone"),
    (".buildkite", "Buildkite"),
];
/// License texts by telltale phrases (all must appear), most specific first.
const LICENSES: &[(&str, &[&str])] = &[
    ("AGPL-3.0", &["GNU AFFERO GENERAL PUBLIC LICENSE"]),
    ("LGPL", &["GNU LESSER GENERAL PUBLIC LICENSE"]),
    ("GPL-3.0", &["GNU GENERAL PUBLIC LICENSE", "Version 3"]),
    ("GPL-2.0", &["GNU GENERAL PUBLIC LICENSE", "Version 2"]),
    ("Apache-2.0", &["Apache License", "Version 2.0"]),
    ("MPL-2.0", &["Mozilla Public License"]),
    ("BSL-1.0", &["Boost Software License"]),
    ("MIT", &["Permission is hereby granted, free of charge"]),
    (
        "ISC",
        &["Permission to use, copy, modify, and/or distribute"],
    ),
    (
        "BSD-3-Clause",
        &[
            "Redistribution and use in source and binary forms",
            "Neither the name",
        ],
    ),
    (
        "BSD-2-Clause",
        &["Redistribution and use in source and binary forms"],
    ),
    ("Unlicense", &["This is free and unencumbered software"]),
    ("Zlib", &["This software is provided 'as-is'"]),
];

/// The build system a file (or folder) name marks, if any.
pub fn build_system_of(file_name: &str) -> Option<&'static str> {
    let extension = file_name.rsplit_once('.').map(|(_, e)| e);
    BUILD_SYSTEMS
        .iter()
        .find(|(_, names, extensions)| {
            names.contains(&file_name) || extension.is_some_and(|e| extensions.contains(&e))
        })
        .map(|(system, _, _)| *system)
}

/// Whether a top-level entry is documentation to read first.
pub fn is_top_level_doc(name: &str, is_dir: bool) -> bool {
    if is_dir {
        return DOC_DIRS.contains(&name.to_ascii_lowercase().as_str());
    }
    let upper = name.to_ascii_uppercase();
    DOC_PREFIXES.iter().any(|p| upper.starts_with(p))
}

/// Whether a top-level file holds the license text.
pub fn is_license_file(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["LICENSE", "LICENCE", "COPYING"]
        .iter()
        .any(|p| upper.starts_with(p))
}

/// The CI service a top-level entry configures, if any.
pub fn ci_of(name: &str) -> Option<&'static str> {
    CI_MARKERS
        .iter()
        .find(|(marker, _)| *marker == name)
        .map(|(_, service)| *service)
}

/// The license `text` is, if it is a well-known one.
pub fn identify_license(text: &str) -> Option<&'static str> {
    LICENSES
        .iter()
        .find(|(_, phrases)| phrases.iter().all(|p| text.contains(p)))
        .map(|(license, _)| *license)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_build_files_by_name_and_extension() {
        assert_eq!(build_system_of("CMakeLists.txt"), Some("CMake"));
        assert_eq!(build_system_of("configure.ac"), Some("Autotools"));
        assert_eq!(
            build_system_of("app.vcxproj"),
            Some("Visual Studio / MSBuild")
        );
        assert_eq!(build_system_of("App.xcodeproj"), Some("Xcode"));
        assert_eq!(build_system_of("main.c"), None);
    }

    #[test]
    fn recognizes_docs_and_ci() {
        assert!(is_top_level_doc("README.md", false));
        assert!(is_top_level_doc("Copying", false));
        assert!(is_top_level_doc("docs", true));
        assert!(!is_top_level_doc("src", true));
        assert!(!is_top_level_doc("main.c", false));
        assert!(is_license_file("LICENSE.txt") && !is_license_file("README"));
        assert_eq!(ci_of(".github"), Some("GitHub Actions"));
        assert_eq!(ci_of("src"), None);
    }

    #[test]
    fn identifies_common_licenses() {
        let bsd3 = "Redistribution and use in source and binary forms ... Neither the name of";
        assert_eq!(identify_license(bsd3), Some("BSD-3-Clause"));
        let gpl2 = "GNU GENERAL PUBLIC LICENSE\n Version 2, June 1991";
        assert_eq!(identify_license(gpl2), Some("GPL-2.0"));
        let lgpl = "GNU LESSER GENERAL PUBLIC LICENSE Version 2.1";
        assert_eq!(identify_license(lgpl), Some("LGPL"));
        let mit = "MIT License\n\nPermission is hereby granted, free of charge, to any person";
        assert_eq!(identify_license(mit), Some("MIT"));
        assert_eq!(identify_license("All rights reserved."), None);
    }
}
