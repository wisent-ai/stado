use super::{flag, native::forwarded, value};
use clap::{Arg, Command};

pub(super) fn command() -> Command {
    Command::new("swift")
        .about("Build and index canonical Swift sources, restore release dependencies, or serve SourceKit settings")
        .arg(value("package-path", "Package directory; defaults to the current directory"))
        .arg(value("editor-workspace", "Separate editor workspace for index declarations"))
        .arg(value("archive", "Portable SwiftPM input archive to restore without compiling")
            .required_if_eq("operation", "restore"))
        .arg(flag("json", "Print source, execution or restoration evidence"))
        .arg(Arg::new("operation").required(true).value_parser([
            "build", "test", "run", "index", "sourcekit", "restore",
        ]))
        .arg(forwarded())
}
