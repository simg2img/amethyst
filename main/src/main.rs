use std::ffi::CString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn write_file(path: &str, val: &str) {
    if let Ok(mut f) = fs::OpenOptions::new().write(true).open(path) {
        let _ = f.write_all(val.as_bytes());
    }
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

fn throttle(disable: bool) {
    let rc_dirs = ["/system/etc/init", "/vendor/etc/init", "/odm/etc/init"];
    let mut services: Vec<String> = Vec::new();

    for dir in &rc_dirs {
        let dir_path = Path::new(dir);
        if !dir_path.is_dir() {
            continue;
        }
        if let Ok(entries) = fs::read_dir(dir_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("rc") {
                    continue;
                }
                if let Ok(content) = fs::read_to_string(&path) {
                    for line in content.lines() {
                        if let Some(svc) = line.strip_prefix("service ") {
                            if let Some(name) = svc.split_whitespace().next() {
                                if !name.is_empty()
                                    && (name.contains("thermal") || name.contains("therm"))
                                {
                                    services.push(name.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    for svc in &services {
        let action = if disable { "stop" } else { "start" };
        let _ = Command::new("su")
            .args([
                "-lp",
                "2000",
                "-c",
                &format!("resetprop -n ctl.{} {}", action, svc),
            ])
            .output();
    }

    if let Ok(entries) = fs::read_dir("/sys/class/thermal/") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("thermal_zone") {
                continue;
            }

            let zone_path = entry.path();
            if let Ok(trip_entries) = fs::read_dir(&zone_path) {
                for trip in trip_entries.flatten() {
                    let trip_name = trip.file_name();
                    let trip_str = trip_name.to_string_lossy();
                    if !trip_str.starts_with("trip_point_") || !trip_str.ends_with("_temp") {
                        continue;
                    }
                    let trip_path = trip.path();
                    let _ = fs::set_permissions(&trip_path, fs::Permissions::from_mode(0o644));
                    write_file(
                        trip_path.to_str().unwrap_or(""),
                        if disable { "125000" } else { "45000" },
                    );
                    let _ = fs::set_permissions(&trip_path, fs::Permissions::from_mode(0o444));
                }

                let mode_path = zone_path.join("mode");
                if mode_path.exists() {
                    write_file(
                        mode_path.to_str().unwrap_or(""),
                        if disable { "disabled" } else { "enabled" },
                    );
                }
            }
        }
    }

    let ppm_content = match fs::read_to_string("/proc/ppm/policy_status") {
        Ok(c) => c,
        Err(_) => return,
    };

    for line in ppm_content.lines() {
        if line.contains("PPM_POLICY_PWR_THRO") || line.contains("PPM_POLICY_THERMAL") {
            if let Some(ob) = line.find('[') {
                if let Some(cb) = line[ob..].find(']') {
                    let idx = &line[ob + 1..ob + cb];
                    if let Ok(mut f) =
                        fs::OpenOptions::new().write(true).open("/proc/ppm/policy_status")
                    {
                        let cmd = format!("{} {}\n", idx, if disable { 0 } else { 1 });
                        let _ = f.write_all(cmd.as_bytes());
                    }
                }
            }
        }
    }
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
            throttle(true);
            was_perf_mode = true;
        } else if !is_perf_mode && was_perf_mode {
            throttle(false);
            was_perf_mode = false;
        }

        std::thread::sleep(Duration::from_secs(1));
    }
}
