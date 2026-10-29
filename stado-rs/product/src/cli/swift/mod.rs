use super::{flag, native::forwarded, value};
use clap::{Arg, Command};

pub(super) fn command() -> Command {
    Command::new("swift")
        .about("Build and index canonical Swift sources, restore release dependencies, or (api) refuse a package revision whose version tag disagrees with what it did to the library's public API, as swift api-digester reads it against the released tag")
        .arg(value(
            "package-path",
            "Package directory; defaults to the current directory",
        ))
        .arg(value(
            "editor-workspace",
            "Separate editor workspace for index declarations",
        ))
        .arg(
            value(
                "archive",
                "Portable SwiftPM input archive to restore without compiling",
            )
            .required_if_eq("operation", "restore"),
        )
        .arg(value(
            "module",
            "api: the library module to read; defaults to the manifest's single library product",
        ))
        .arg(flag(
            "json",
            "Print source, execution or restoration evidence; for api, the dumps, the digester's diagnosis and the verdict",
        ))
        .arg(
            Arg::new("operation")
                .required(true)
                .value_parser(["build", "test", "run", "index", "restore", "api"]),
        )
        .arg(forwarded())
}
