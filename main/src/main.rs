use std::ffi::CString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn write_file(path: &str, val: &str) -> bool {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|mut f| f.write_all(val.as_bytes()))
        .is_ok()
}

fn read_file<'a>(path: &str, buf: &'a mut [u8]) -> &'a str {
    match fs::OpenOptions::new().read(true).open(path) {
        Ok(mut f) => match f.read(buf) {
            Ok(0) | Err(_) => "",
            Ok(n) => {
                let end = buf[..n]
                    .iter()
                    .rposition(|&b| b != b'\n' && b != b'\r')
                    .map_or(0, |i| i + 1);
                std::str::from_utf8(&buf[..end]).unwrap_or("")
            }
        },
        Err(_) => "",
    }
}

fn update_prop_status(status: &str) {
    let path = "/data/adb/modules/amethyst/module.prop";
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    if let Some(pos) = content.find("description=") {
        let eq = pos + "description=".len();
        let end = content[eq..]
            .find('\n')
            .map(|i| eq + i)
            .unwrap_or(content.len());
        let new_desc = format!(
            "description=[{}] A lightweight thermal disabler for Android",
            status
        );

        let mut new_content = String::with_capacity(content.len() + 64);
        new_content.push_str(&content[..pos]);
        new_content.push_str(&new_desc);
        new_content.push_str(&content[end..]);

        if let Ok(mut f) = fs::OpenOptions::new().write(true).truncate(true).open(path) {
            let _ = f.write_all(new_content.as_bytes());
        }
    }
}

fn set_thermal_zones(disable: bool) -> (usize, usize, Vec<String>) {
    let mut ok = 0;
    let mut total = 0;
    let mut errors = Vec::new();

    let dir = Path::new("/sys/class/thermal/");
    if !dir.is_dir() {
        return (0, 0, errors);
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return (0, 0, errors),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("thermal_zone") {
            continue;
        }

        total += 1;
        let zone_path = entry.path();
        let mode_path = zone_path.join("mode");
        let _zone_type = read_file(
            zone_path.join("type").to_str().unwrap_or(""),
            &mut [0u8; 64],
        )
        .to_string();

        if mode_path.exists() {
            let target = if disable { "disabled" } else { "enabled" };
            if write_file(mode_path.to_str().unwrap_or(""), target) {
                std::thread::sleep(Duration::from_millis(50));
                let mut buf = [0u8; 16];
                let cur = read_file(mode_path.to_str().unwrap_or(""), &mut buf);
                if cur.trim() == target {
                    ok += 1;
                } else {
                    errors.push(format!("{} mode verify fail", name_str));
                }
            } else {
                errors.push(format!("{} mode write fail", name_str));
            }
        }
    }

    (ok, total, errors)
}

fn set_trip_temps(disable: bool) -> (usize, usize, Vec<String>) {
    let mut ok = 0;
    let mut total = 0;
    let mut errors = Vec::new();

    let dir = Path::new("/sys/class/thermal/");
    if !dir.is_dir() {
        return (0, 0, errors);
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return (0, 0, errors),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("thermal_zone") {
            continue;
        }

        let zone_path = entry.path();
        let trip_entries = match fs::read_dir(&zone_path) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for trip in trip_entries.flatten() {
            let trip_name = trip.file_name();
            let trip_str = trip_name.to_string_lossy();
            if !trip_str.starts_with("trip_point_") || !trip_str.ends_with("_temp") {
                continue;
            }

            total += 1;
            let trip_path = trip.path();
            let trip_path_str = match trip_path.to_str() {
                Some(s) => s,
                None => continue,
            };

            let target = if disable { "125000" } else { "45000" };

            let _ = fs::set_permissions(&trip_path, fs::Permissions::from_mode(0o644));
            if write_file(trip_path_str, target) {
                let mut buf = [0u8; 16];
                let cur = read_file(trip_path_str, &mut buf);
                if cur.trim() == target {
                    ok += 1;
                } else {
                    errors.push(format!("{} verify fail (got {})", trip_str, cur.trim()));
                }
            } else {
                errors.push(format!("{} write fail", trip_str));
            }
            let _ = fs::set_permissions(&trip_path, fs::Permissions::from_mode(0o444));
        }
    }

    (ok, total, errors)
}

fn manage_cooling_devices(disable: bool) -> (usize, usize, Vec<String>) {
    let mut ok = 0;
    let mut total = 0;
    let mut errors = Vec::new();

    let dir = Path::new("/sys/class/thermal/");
    if !dir.is_dir() {
        return (0, 0, errors);
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return (0, 0, errors),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("cooling_device") {
            continue;
        }

        total += 1;
        let dev_path = entry.path();
        let cur_path = dev_path.join("cur_state");
        let max_path = dev_path.join("max_state");
        let _dev_type = read_file(
            dev_path.join("type").to_str().unwrap_or(""),
            &mut [0u8; 64],
        )
        .to_string();

        if cur_path.exists() {
            let target = if disable {
                let mut buf = [0u8; 16];
                let max_str = read_file(max_path.to_str().unwrap_or(""), &mut buf);
                max_str.trim().to_string()
            } else {
                "0".to_string()
            };

            if write_file(cur_path.to_str().unwrap_or(""), &target) {
                let mut buf = [0u8; 16];
                let cur = read_file(cur_path.to_str().unwrap_or(""), &mut buf);
                if cur.trim() == target.trim() {
                    ok += 1;
                } else {
                    errors.push(format!("{} cur_state verify fail", name_str));
                }
            } else {
                errors.push(format!("{} cur_state write fail", name_str));
            }
        }
    }

    (ok, total, errors)
}

fn manage_thermal_services(disable: bool) -> Vec<String> {
    let known = [
        "thermal",
        "thermald",
        "thermal_manager",
        "thermal-engine",
        "vendor.thermal-hal-2-0",
        "vendor.thermal-hal-2-0.mtk",
        "vendor.thermal-hal-1-0",
        "vendor.thermal-hal-1-1",
        "vendor.thermal-hal-1-2",
        "vendor.thermal-hal-1-3",
        "vendor.thermal-hal-1-4",
        "power-hal",
        "power_hal",
    ];

    let mut results = Vec::new();

    for svc in &known {
        let action = if disable { "stop" } else { "start" };
        let out = Command::new("resetprop")
            .args(["-n", &format!("ctl.{}", action), svc])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                results.push(format!("{}:{}", action, svc));
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }

    results
}

fn manage_mtk_ppm(disable: bool) -> (usize, usize, Vec<String>) {
    let ppm_path = "/proc/ppm/policy_status";
    let content = match fs::read_to_string(ppm_path) {
        Ok(c) => c,
        Err(_) => return (0, 0, Vec::new()),
    };

    let mut ok = 0;
    let mut total = 0;
    let mut errors = Vec::new();
    let val = if disable { 0u32 } else { 1 };

    for line in content.lines() {
        let is_thermal = line.contains("PPM_POLICY_PWR_THRO")
            || line.contains("PPM_POLICY_THERMAL");
        let is_force = line.contains("PPM_POLICY_FORCE_LIMIT");
        let is_dlpt = line.contains("PPM_POLICY_DLPT");

        if !is_thermal && !is_force && !is_dlpt {
            continue;
        }

        total += 1;
        if let Some(ob) = line.find('[') {
            if let Some(cb) = line[ob..].find(']') {
                let idx = &line[ob + 1..ob + cb];
                if let Ok(mut f) = fs::OpenOptions::new().write(true).open(ppm_path) {
                    let cmd = format!("{} {}\n", idx, if is_thermal { val } else { 1u32 });
                    if f.write_all(cmd.as_bytes()).is_ok() {
                        ok += 1;
                    } else {
                        errors.push(format!("ppm policy {} write fail", idx));
                    }
                }
            }
        }
    }

    (ok, total, errors)
}

fn manage_mtk_legacy(disable: bool) -> (usize, usize, Vec<String>) {
    let mut ok = 0;
    let mut total = 0;
    let mut errors = Vec::new();

    let paths = [
        "/sys/devices/virtual/thermal/thermal_message/cpu_limits",
        "/sys/kernel/fpsgo/fbt/thrm_limit_cpu",
        "/sys/kernel/fpsgo/fbt/thrm_temp_th",
    ];

    for p in &paths {
        let path = Path::new(p);
        if path.exists() {
            total += 1;
            if disable {
                if write_file(p, "2000000") {
                    ok += 1;
                } else {
                    errors.push(format!("{} write fail", p));
                }
            } else {
                if write_file(p, "0") {
                    ok += 1;
                } else {
                    errors.push(format!("{} restore fail", p));
                }
            }
        }
    }

    (ok, total, errors)
}

fn throttle(disable: bool) -> String {
    let mut parts: Vec<String> = Vec::new();

    let (z_ok, z_total, z_err) = set_thermal_zones(disable);
    parts.push(format!("zones:{}/{}", z_ok, z_total));

    let (t_ok, t_total, t_err) = set_trip_temps(disable);
    parts.push(format!("trips:{}/{}", t_ok, t_total));

    let (c_ok, c_total, c_err) = manage_cooling_devices(disable);
    parts.push(format!("cool:{}/{}", c_ok, c_total));

    let svc_list = manage_thermal_services(disable);
    parts.push(format!("svc:{}", svc_list.len()));

    let (p_ok, p_total, p_err) = manage_mtk_ppm(disable);
    if p_total > 0 {
        parts.push(format!("ppm:{}/{}", p_ok, p_total));
    }

    let (l_ok, l_total, l_err) = manage_mtk_legacy(disable);
    if l_total > 0 {
        parts.push(format!("legacy:{}/{}", l_ok, l_total));
    }

    let all_errors: Vec<&str> = z_err
        .iter()
        .chain(t_err.iter())
        .chain(c_err.iter())
        .chain(p_err.iter())
        .chain(l_err.iter())
        .map(|s| s.as_str())
        .collect();

    if all_errors.is_empty() {
        parts.push("ok".to_string());
    } else {
        parts.push(format!("err:{}", all_errors.len()));
    }

    parts.join(" ")
}

fn main() {
    let child_pid = unsafe { libc::fork() };

    if child_pid < 0 {
        update_prop_status("\u{274c}daemon (fork failed)");
        return;
    }

    if child_pid > 0 {
        std::thread::sleep(Duration::from_secs(2));

        let mut status: libc::c_int = 0;
        let ret = unsafe { libc::waitpid(child_pid, &mut status, libc::WNOHANG) };
        if ret == child_pid {
            update_prop_status("\u{274c}daemon (exited)");
            return;
        }

        if unsafe { libc::kill(child_pid, 0) } != 0 {
            update_prop_status("\u{274c}daemon (died)");
            return;
        }

        update_prop_status(&format!("\u{2705}daemon ({})", child_pid));
        return;
    }

    unsafe {
        libc::setsid();
    }
    let _ = std::env::set_current_dir("/");
    unsafe {
        libc::umask(0);
    }

    for i in 0..3 {
        unsafe {
            libc::close(i);
        }
    }

    let null_path = CString::new("/dev/null").unwrap();
    let null_fd = unsafe { libc::open(null_path.as_ptr(), libc::O_RDWR) };
    if null_fd >= 0 {
        unsafe {
            libc::dup(null_fd);
            libc::dup(null_fd);
        }
    }

    let mut was_perf_mode = false;

    loop {
        let mut governor_buf = [0u8; 32];
        let governor = read_file(
            "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor",
            &mut governor_buf,
        );

        let is_perf_mode = governor == "performance";

        if is_perf_mode && !was_perf_mode {
            let report = throttle(true);
            update_prop_status(&format!(
                "\u{2705}block {}",
                report
            ));
            was_perf_mode = true;
        } else if !is_perf_mode && was_perf_mode {
            let report = throttle(false);
            update_prop_status(&format!(
                "\u{2705}unblock {}",
                report
            ));
            was_perf_mode = false;
        }

        std::thread::sleep(Duration::from_secs(1));
    }
}
