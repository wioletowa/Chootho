//! Keeps the Vulkan loader from pulling in drivers Chootho doesn't need.
//!
//! The loader opens every installed driver at startup. On a hybrid laptop
//! that means the NVIDIA driver (~100 MB mapped, and it wakes the dGPU) plus
//! lavapipe's LLVM (~80 MB), for an app that an integrated GPU draws fine.

use std::ffi::OsString;
use std::path::PathBuf;

/// Vulkan driver manifest directories, as the loader searches them.
fn manifest_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/etc/vulkan/icd.d")];
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".local/share")));
    dirs.extend(data_home.map(|d| d.join("vulkan/icd.d")));
    let data_dirs = std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| OsString::from("/usr/local/share:/usr/share"));
    dirs.extend(std::env::split_paths(&data_dirs).map(|d| d.join("vulkan/icd.d")));
    dirs
}

/// Disables lavapipe whenever a real GPU driver exists, and NVIDIA whenever
/// another GPU driver exists. Leaves everything alone if the user already
/// chose drivers through the loader's own variables. Must run before any
/// thread starts.
pub fn prefer_integrated() {
    const USER_VARS: [&str; 4] =
        ["VK_DRIVER_FILES", "VK_ICD_FILENAMES", "VK_LOADER_DRIVERS_SELECT", "VK_LOADER_DRIVERS_DISABLE"];
    if USER_VARS.iter().any(|v| std::env::var_os(v).is_some()) {
        return;
    }
    let names: Vec<String> = manifest_dirs()
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".json"))
        .collect();
    let software = |n: &String| n.contains("lvp");
    let nvidia = |n: &String| n.contains("nvidia");

    let mut disable = Vec::new();
    if names.iter().any(|n| !software(n)) && names.iter().any(software) {
        disable.push("*lvp*");
    }
    if names.iter().any(|n| !software(n) && !nvidia(n)) && names.iter().any(nvidia) {
        disable.push("*nvidia*");
    }
    if !disable.is_empty() {
        // SAFETY: called first thing in main, before GTK or GStreamer start threads.
        unsafe { std::env::set_var("VK_LOADER_DRIVERS_DISABLE", disable.join(",")) };
    }
}
