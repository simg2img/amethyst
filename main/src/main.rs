use std::ffi::CString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn write_file(path: &str, val: &str) -> bool {
    if !Path::new(path).exists() {
        return false;
    }
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|mut f| f.write_all(val.as_bytes()))
        .is_ok()
}

fn write_verify(path: &str, val: &str) -> bool {
    if !write_file(path, val) {
        return false;
    }
    std::thread::sleep(Duration::from_millis(20));
    let mut buf = [0u8; 64];
    let cur = read_file(path, &mut buf);
    cur.trim() == val.trim()
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
            "description= {} | A lightweight thermal disabler for Android",
            status
        );

        let mut new_content = String::with_capacity(content.len() + 128);

        new_content.push_str(&content[..pos]);
        new_content.push_str(&new_desc);
        new_content.push_str(&content[end..]);

        if let Ok(mut f) = fs::OpenOptions::new().write(true).truncate(true).open(path) {
            let _ = f.write_all(new_content.as_bytes());
        }
    }
}

struct Op {
    ok: usize,
    total: usize,
    errors: Vec<String>,
}

impl Op {
    fn new() -> Self {
        Op { ok: 0, total: 0, errors: Vec::new() }
    }

    fn write(&mut self, path: &str, val: &str) {
        self.total += 1;
        if write_verify(path, val) {
            self.ok += 1;
        } else {
            self.errors.push(format!("{} fail", path));
        }
    }

    fn write_noverify(&mut self, path: &str, val: &str) {
        self.total += 1;
        if write_file(path, val) {
            self.ok += 1;
        } else {
            self.errors.push(format!("{} fail", path));
        }
    }

    fn done(self) -> (usize, usize, Vec<String>) {
        (self.ok, self.total, self.errors)
    }
}

fn set_thermal_zones(disable: bool) -> (usize, usize, Vec<String>) {
    let mut op = Op::new();
    let target_mode = if disable { "disabled" } else { "enabled" };
    let target_policy = if disable { "user_space" } else { "" };

    let dir = Path::new("/sys/class/thermal/");
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return op.done(),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("thermal_zone") {
            continue;
        }

        let zone_path = entry.path();
        let zpath = |s: &str| zone_path.join(s).to_str().unwrap_or("").to_string();

        op.write(&zpath("mode"), target_mode);

        if disable {
            let _ = write_file(&zpath("policy"), target_policy);
            let _ = write_file(&zpath("sustainable_power"), "50000");
            let _ = write_file(&zpath("k_po"), "0");
            let _ = write_file(&zpath("k_pu"), "0");
            let _ = write_file(&zpath("k_i"), "0");
            let _ = write_file(&zpath("k_d"), "0");
            let _ = write_file(&zpath("integral_cutoff"), "0");
        }
    }

    op.done()
}

fn set_trip_temps(disable: bool) -> (usize, usize, Vec<String>) {
    let mut op = Op::new();
    let target = if disable { "125000" } else { "45000" };

    let dir = Path::new("/sys/class/thermal/");
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return op.done(),
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

            let trip_path = trip.path();
            let trip_path_str = match trip_path.to_str() {
                Some(s) => s,
                None => continue,
            };

            let _ = fs::set_permissions(&trip_path, fs::Permissions::from_mode(0o644));
            if write_verify(trip_path_str, target) {
                op.ok += 1;
            } else {
                op.errors.push(format!("{} fail", trip_str));
            }
            op.total += 1;
            let _ = fs::set_permissions(&trip_path, fs::Permissions::from_mode(0o444));
        }
    }

    op.done()
}

fn manage_cooling_devices(disable: bool) -> (usize, usize, Vec<String>) {
    let mut op = Op::new();

    let dir = Path::new("/sys/class/thermal/");
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return op.done(),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("cooling_device") {
            continue;
        }

        let dev_path = entry.path();
        let cur_path = dev_path.join("cur_state");
        let max_path = dev_path.join("max_state");
        let cpath = |s: &str| dev_path.join(s).to_str().unwrap_or("").to_string();

        if !cur_path.exists() {
            continue;
        }

        let val = if disable {
            let mut buf = [0u8; 16];
            read_file(max_path.to_str().unwrap_or(""), &mut buf).trim().to_string()
        } else {
            "0".to_string()
        };

        if val.is_empty() || val == "0" {
            op.write(&cpath("cur_state"), "0");
        } else {
            op.write(&cpath("cur_state"), &val);
        }
    }

    op.done()
}

fn manage_msm_thermal(disable: bool) -> (usize, usize, Vec<String>) {
    let mut op = Op::new();

    if disable {
        op.write("/sys/module/msm_thermal/parameters/enabled", "N");
        op.write("/sys/module/msm_thermal/parameters/core_control_enabled", "0");
        op.write("/sys/module/msm_thermal/parameters/freq_mitigation_enabled", "0");
        op.write("/sys/module/msm_thermal/parameters/vdd_restriction_enabled", "0");
        op.write("/sys/module/msm_thermal/parameters/mx_restriction_enabled", "0");
        op.write("/sys/module/msm_thermal/parameters/limit_temp_degC", "150");
        op.write("/sys/module/msm_thermal/parameters/therm_reset_temp_degC", "150");
        op.write("/sys/module/msm_thermal/parameters/poll_ms", "0");
        op.write("/sys/module/msm_thermal/parameters/temp_hysteresis_degC", "0");
        op.write("/sys/module/msm_thermal/parameters/freq_step", "0");
    } else {
        op.write("/sys/module/msm_thermal/parameters/enabled", "Y");
        op.write("/sys/module/msm_thermal/parameters/core_control_enabled", "1");
        op.write("/sys/module/msm_thermal/parameters/freq_mitigation_enabled", "1");
        op.write("/sys/module/msm_thermal/parameters/vdd_restriction_enabled", "1");
        op.write("/sys/module/msm_thermal/parameters/mx_restriction_enabled", "1");
        op.write("/sys/module/msm_thermal/parameters/limit_temp_degC", "60");
        op.write("/sys/module/msm_thermal/parameters/therm_reset_temp_degC", "60");
        op.write("/sys/module/msm_thermal/parameters/poll_ms", "1000");
        op.write("/sys/module/msm_thermal/parameters/temp_hysteresis_degC", "5");
        op.write("/sys/module/msm_thermal/parameters/freq_step", "1");
    }

    op.done()
}

fn manage_gpu_thermal(disable: bool) -> (usize, usize, Vec<String>) {
    let mut op = Op::new();

    let gpu = "/sys/class/kgsl/kgsl-3d0";

    if disable {
        op.write(&format!("{}/thermal_pwrlevel", gpu), "0");
        op.write(&format!("{}/max_pwrlevel", gpu), "0");
        op.write(&format!("{}/throttling", gpu), "0");
        op.write(&format!("{}/force_rail_on", gpu), "1");
        op.write(&format!("{}/force_clk_on", gpu), "1");
        op.write(&format!("{}/force_no_nap", gpu), "1");
        op.write(&format!("{}/bus_split", gpu), "0");
        let _ = write_file(&format!("{}/idle_timer", gpu), "10000");

        let _ = write_file("/sys/module/adreno_idler/parameters/adreno_idler_active", "0");
    } else {
        op.write(&format!("{}/throttling", gpu), "1");
        op.write(&format!("{}/force_rail_on", gpu), "0");
        op.write(&format!("{}/force_clk_on", gpu), "0");
        op.write(&format!("{}/force_no_nap", gpu), "0");
        op.write(&format!("{}/bus_split", gpu), "1");
        let _ = write_file(&format!("{}/thermal_pwrlevel", gpu), "1");
    }

    op.done()
}

fn manage_core_ctl(disable: bool) -> (usize, usize, Vec<String>) {
    let mut op = Op::new();
    let clusters = ["cpu0", "cpu4", "cpu5", "cpu6", "cpu7"];

    if disable {
        let _ = write_file("/sys/module/core_ctl/parameters/enable", "N");
        for c in &clusters {
            op.write(&format!("/sys/devices/system/cpu/{}/core_ctl/enable", c), "0");
        }
    } else {
        let _ = write_file("/sys/module/core_ctl/parameters/enable", "Y");
        for c in &clusters {
            let _ = write_file(&format!("/sys/devices/system/cpu/{}/core_ctl/enable", c), "1");
        }
    }

    op.done()
}

fn manage_devfreq(disable: bool) -> (usize, usize, Vec<String>) {
    let mut op = Op::new();
    let dir = Path::new("/sys/class/devfreq/");
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return op.done(),
    };

    for entry in entries.flatten() {
        let dev_path = entry.path();
        let dpath = |s: &str| dev_path.join(s).to_str().unwrap_or("").to_string();

        if disable {
            let _ = write_file(&dpath("governor"), "userspace");
            let mut buf = [0u8; 32];
            let cur = read_file(&dpath("cur_freq"), &mut buf);
            let max = cur.trim();
            if !max.is_empty() {
                op.write(&dpath("max_freq"), max);
            }
            let _ = write_file(&dpath("polling_interval"), "0");
        } else {
            let _ = write_file(&dpath("governor"), "performance");
        }
    }

    op.done()
}

fn manage_mtk_ppm(disable: bool) -> (usize, usize, Vec<String>) {
    let content = match fs::read_to_string("/proc/ppm/policy_status") {
        Ok(c) => c,
        Err(_) => return (0, 0, Vec::new()),
    };

    let mut op = Op::new();
    let therm_val = if disable { 0u32 } else { 1 };

    for line in content.lines() {
        let is_therm = line.contains("PPM_POLICY_PWR_THRO")
            || line.contains("PPM_POLICY_THERMAL");
        let is_force = line.contains("PPM_POLICY_FORCE_LIMIT");
        let is_dlpt = line.contains("PPM_POLICY_DLPT");

        if !is_therm && !is_force && !is_dlpt {
            continue;
        }

        if let Some(ob) = line.find('[') {
            if let Some(cb) = line[ob..].find(']') {
                let idx = &line[ob + 1..ob + cb];
                let cmd = format!("{} {}\n", idx, if is_therm { therm_val } else { 1u32 });
                op.write_noverify("/proc/ppm/policy_status", &cmd);
            }
        }
    }

    op.done()
}

fn manage_mtk_thermal(disable: bool) -> (usize, usize, Vec<String>) {
    let mut op = Op::new();

    let mtk_enable = if disable { "0" } else { "1" };
    let mtk_high = if disable { "2000000" } else { "0" };

    op.write_noverify("/proc/driver/mtk_thermal_monitor", mtk_enable);
    op.write_noverify("/proc/cpufreq/cpufreq_power_cap", mtk_high);
    op.write_noverify("/sys/devices/virtual/thermal/thermal_message/cpu_limits", "cpu0 2000000");
    op.write_noverify("/sys/kernel/fpsgo/fbt/thrm_limit_cpu", "2000000");
    op.write_noverify("/sys/kernel/fpsgo/fbt/thrm_temp_th", "200000");

    let perfmgr = Path::new("/proc/perfmgr/thermal/");
    if perfmgr.is_dir() {
        if let Ok(entries) = fs::read_dir(perfmgr) {
            for e in entries.flatten() {
                let p = e.path();
                let ps = match p.to_str() {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                op.write_noverify(&ps, mtk_enable);
            }
        }
    }

    let mtkcooler = Path::new("/proc/mtkcooler/");
    if mtkcooler.is_dir() {
        if let Ok(entries) = fs::read_dir(mtkcooler) {
            for e in entries.flatten() {
                let p = e.path();
                let ps = match p.to_str() {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let mut buf = [0u8; 16];
                let cur = read_file(&ps, &mut buf);
                if cur.trim() == "0" || cur.trim() == "1" {
                    op.write_noverify(&ps, mtk_enable);
                }
            }
        }
    }

    op.done()
}

fn manage_module_params(disable: bool) -> (usize, usize, Vec<String>) {
    let mut op = Op::new();

    let mod_dir = Path::new("/sys/module/");
    let entries = match fs::read_dir(mod_dir) {
        Ok(e) => e,
        Err(_) => return op.done(),
    };

    for entry in entries.flatten() {
        let mod_name = entry.file_name();
        let mod_str = mod_name.to_string_lossy();
        let is_thermal = mod_str.contains("thermal")
            || mod_str.contains("therm")
            || mod_str.contains("cooling")
            || mod_str == "core_ctl"
            || mod_str == "adreno_idler";

        if !is_thermal {
            continue;
        }

        let params_dir = entry.path().join("parameters");
        if !params_dir.is_dir() {
            continue;
        }

        if let Ok(params) = fs::read_dir(&params_dir) {
            for param in params.flatten() {
                let pname = param.file_name();
                let _pstr = pname.to_string_lossy();
                let ppath = match param.path().to_str() {
                    Some(s) => s.to_string(),
                    None => continue,
                };

                if mod_str == "msm_thermal" {
                    continue;
                }

                let mut buf = [0u8; 32];
                let cur = read_file(&ppath, &mut buf);
                let trimmed = cur.trim();

                if disable {
                    if trimmed == "Y" || trimmed == "1" || trimmed == "enabled" || trimmed == "true" {
                        op.write(&ppath, "N");
                    } else if trimmed == "0" {
                        op.total += 1;
                        op.ok += 1;
                    } else if let Ok(v) = trimmed.parse::<u32>() {
                        if v > 0 && v < 10000 {
                            op.write(&ppath, "0");
                        } else {
                            op.total += 1;
                            op.ok += 1;
                        }
                    }
                } else {
                    if trimmed == "N" || trimmed == "0" || trimmed == "disabled" || trimmed == "false" {
                        op.write(&ppath, "Y");
                    }
                }
            }
        }
    }

    op.done()
}

fn manage_thermal_services(disable: bool) -> usize {
    let mut count = 0;

    let output = match Command::new("resetprop").output() {
        Ok(o) => match String::from_utf8(o.stdout) {
            Ok(s) => s,
            Err(_) => return 0,
        },
        Err(_) => return 0,
    };

    for line in output.lines() {
        if !line.contains("running") {
            continue;
        }
        if let Some(pos) = line.find("init.svc.") {
            let after = &line[pos + "init.svc.".len()..];
            let svc_name = after.split(|c| c == ']' || c == ':' || c == '[' || c == ' ')
                .next()
                .unwrap_or("")
                .trim();
            if svc_name.is_empty() || !svc_name.contains("thermal") {
                continue;
            }
            let action = if disable { "stop" } else { "start" };
            if let Ok(o) = Command::new("resetprop")
                .args(["-n", &format!("ctl.{}", action), svc_name])
                .output()
            {
                if o.status.success() {
                    count += 1;
                }
            }
        }
    }

    count
}

fn manage_platform_devices(disable: bool) -> (usize, usize, Vec<String>) {
    let mut op = Op::new();
    let dir = Path::new("/sys/devices/platform/");
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return op.done(),
    };

    for entry in entries.flatten() {
        let dev_name = entry.file_name();
        let dev_str = dev_name.to_string_lossy();

        let is_therm = dev_str.contains("thermal")
            || dev_str.contains("tmu")
            || dev_str.contains("therm")
            || dev_str == "bcl"
            || dev_str.starts_with("qcom,bcl")
            || dev_str.starts_with("sprd-thermal");

        if !is_therm {
            continue;
        }

        let dev_path = entry.path();
        for sub in ["mode", "enable", "enabled"].iter() {
            let p = dev_path.join(sub);
            if p.exists() {
                let ps = p.to_str().unwrap_or("");
                let val = if disable { "disabled" } else { "enabled" };
                if write_verify(ps, val) {
                    op.ok += 1;
                } else {
                    op.errors.push(format!("{} fail", ps));
                }
                op.total += 1;
            }
        }

        let enable_path = dev_path.join("enable");
        if enable_path.exists() {
            let ps = enable_path.to_str().unwrap_or("");
            let val = if disable { "0" } else { "1" };
            if write_verify(ps, val) {
                op.ok += 1;
            } else {
                op.errors.push(format!("{} fail", ps));
            }
            op.total += 1;
        }
    }

    op.done()
}

fn manage_exynos_tmu(disable: bool) -> (usize, usize, Vec<String>) {
    let mut op = Op::new();
    let dev_dir = Path::new("/sys/devices/platform/");
    let entries = match fs::read_dir(dev_dir) {
        Ok(e) => e,
        Err(_) => return op.done(),
    };

    for entry in entries.flatten() {
        let dev_name = entry.file_name();
        let dev_str = dev_name.to_string_lossy();

        if !dev_str.contains("tmu") && !dev_str.contains("TMU") {
            continue;
        }

        let dev_path = entry.path();
        for sub in ["emulation", "emul_temp"] {
            let p = dev_path.join(sub);
            if p.exists() {
                let ps = match p.to_str() {
                    Some(s) => s,
                    None => continue,
                };
                let val = if disable { "0" } else { "-1" };
                if write_file(ps, val) {
                    op.ok += 1;
                } else {
                    op.errors.push(format!("{} fail", ps));
                }
                op.total += 1;
            }
        }
    }

    op.done()
}

fn throttle(disable: bool) -> String {
    let mut total_ok: usize = 0;
    let mut total_all: usize = 0;
    let mut err_count: usize = 0;

    let mut collect = |(ok, all, errs): (usize, usize, Vec<String>)| {
        total_ok += ok;
        total_all += all;
        err_count += errs.len();
    };

    collect(set_thermal_zones(disable));
    collect(set_trip_temps(disable));
    collect(manage_cooling_devices(disable));
    collect(manage_msm_thermal(disable));
    collect(manage_gpu_thermal(disable));
    collect(manage_core_ctl(disable));
    collect(manage_devfreq(disable));
    collect(manage_mtk_ppm(disable));
    collect(manage_mtk_thermal(disable));
    collect(manage_exynos_tmu(disable));
    collect(manage_platform_devices(disable));
    collect(manage_module_params(disable));

    let svc_count = manage_thermal_services(disable);

    if err_count > 0 {
        format!(
            "{}ok{}/{}-{}err{}svc",
            if total_all > 0 { "" } else { "-" },
            total_ok, total_all, err_count, svc_count
        )
    } else if total_all > 0 {
        format!("ok{}/{} svc{}", total_ok, total_all, svc_count)
    } else {
        format!("ok svc{}", svc_count)
    }
}

fn main() {
    let child_pid = unsafe { libc::fork() };

    if child_pid < 0 {
        update_prop_status(&format!("\u{274c} daemon (fork failed)"));
        return;
    }

    if child_pid > 0 {
        std::thread::sleep(Duration::from_secs(2));

        let mut status: libc::c_int = 0;
        let ret = unsafe { libc::waitpid(child_pid, &mut status, libc::WNOHANG) };
        if ret == child_pid {
            update_prop_status("\u{274c} daemon (exited)");
            return;
        }

        if unsafe { libc::kill(child_pid, 0) } != 0 {
            update_prop_status("\u{274c} daemon (died)");
            return;
        }

        update_prop_status(&format!("\u{2705} daemon ({})", child_pid));
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

    let _ = throttle(true);

    let mut was_perf_mode = true;

    loop {
        std::thread::sleep(Duration::from_secs(5));
        let mut buf = [0u8; 32];
        let gov = read_file(
            "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor",
            &mut buf,
        );
        let is_perf = gov == "performance";

        if is_perf && !was_perf_mode {
            let _ = throttle(true);
            was_perf_mode = true;
        } else if !is_perf && was_perf_mode {
            let _ = throttle(false);
            was_perf_mode = false;
        }
    }
}
