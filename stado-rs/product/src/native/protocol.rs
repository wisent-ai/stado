pub const STATE: &str = ".build/wisent-native/index";
pub const SETTINGS: &str = ".build/wisent-native/index/build-settings.json";
pub const CONNECTION: &str = ".bsp/wisent-native.json";
pub const SETTINGS_SCHEMA: u32 = 1;
pub const PROVIDER_VERSION: &str = "1";
pub const BSP_VERSION: &str = "2.2.0";
pub const SOURCE_FILE_KIND: u8 = 1;
pub const LOG_ERROR: i32 = 1;
pub const NOT_INITIALIZED: i32 = -32002;
pub const REQUEST_FAILED: i32 = -32001;
pub const METHOD_NOT_FOUND: i32 = -32601;

pub fn language(extension: &str) -> Option<&'static str> {
    match extension {
        "swift" => Some("swift"),
        "c" => Some("c"),
        "C" | "cc" | "cpp" | "cxx" => Some("cpp"),
        "m" => Some("objective-c"),
        "mm" => Some("objective-cpp"),
        _ => None,
    }
}
