use std::{
    fs::File,
    io::{BufRead, BufReader},
};

use anyhow::{anyhow, Context, Result};
use log::{debug, error, info};

use crate::{
    datasource::file_path::*,
    utils::{
        file_operate::{check_read, read_file},
        file_status::{get_status, write_status},
    },
};

fn module_ged_load() -> Result<i32> {
    if !get_status(MODULE_LOAD) {
        return Ok(-1);
    }

    let buf = read_file(MODULE_LOAD, 32)?;
    let load = buf
        .trim()
        .parse::<i32>()
        .with_context(|| format!("Failed to parse GPU load from {MODULE_LOAD}"))?;

    Ok(load)
}

fn module_ged_idle() -> Result<i32> {
    if !get_status(MODULE_IDLE) {
        return module_ged_load();
    }

    let buf = read_file(MODULE_IDLE, 32)?;
    let idle = buf
        .trim()
        .parse::<i32>()
        .with_context(|| format!("Failed to parse GPU idle from {MODULE_IDLE}"))?;

    let load = 100 - idle;
    debug!("module {load}");
    Ok(100 - idle)
}

fn kernel_ged_load() -> Result<i32> {
    if !get_status(KERNEL_LOAD) {
        return module_ged_idle();
    }

    let buf = read_file(KERNEL_LOAD, 32)?;
    let parts: Vec<&str> = buf.split_whitespace().collect();

    if parts.len() >= 3 {
        if let Ok(idle) = parts[2].parse::<i32>() {
            let load = 100 - idle;
            debug!("gedload {load}");
            return Ok(if 100 - idle == 0 {
                module_ged_load()?
            } else {
                100 - idle
            });
        }
    }

    module_ged_idle()
}

fn kernel_debug_ged_load() -> Result<i32> {
    if !get_status(KERNEL_D_LOAD) {
        return kernel_ged_load();
    }

    let buf = read_file(KERNEL_D_LOAD, 32)?;
    let parts: Vec<&str> = buf.split_whitespace().collect();

    if parts.len() >= 3 {
        if let Ok(idle) = parts[2].parse::<i32>() {
            let load = 100 - idle;
            debug!("dbggedload {load}");
            return Ok(if 100 - idle == 0 {
                kernel_ged_load()?
            } else {
                100 - idle
            });
        }
    }

    kernel_ged_load()
}

fn kernel_d_ged_load() -> Result<i32> {
    if !get_status(KERNEL_DEBUG_LOAD) {
        return kernel_debug_ged_load();
    }

    let buf = read_file(KERNEL_DEBUG_LOAD, 32)?;
    let parts: Vec<&str> = buf.split_whitespace().collect();

    if parts.len() >= 3 {
        if let Ok(idle) = parts[2].parse::<i32>() {
            let load = 100 - idle;
            debug!("dgedload {load}");
            return Ok(if 100 - idle == 0 {
                kernel_debug_ged_load()?
            } else {
                100 - idle
            });
        }
    }

    kernel_debug_ged_load()
}

fn mali_load() -> Result<i32> {
    if !get_status(PROC_MALI_LOAD) {
        return kernel_d_ged_load();
    }

    let buf = read_file(PROC_MALI_LOAD, 256)?;

    // Parse "gpu/cljs0/cljs1=XX" format
    if let Some(pos) = buf.find('=') {
        if let Ok(load) = buf[pos + 1..].trim().parse::<i32>() {
            debug!("mali {load}");
            return Ok(if load == 0 {
                kernel_d_ged_load()?
            } else {
                load
            });
        }
    }

    kernel_d_ged_load()
}

fn mtk_load() -> Result<i32> {
    if !get_status(PROC_MTK_LOAD) {
        return mali_load();
    }

    let buf = read_file(PROC_MTK_LOAD, 256)?;

    // Parse "ACTIVE=XX" format
    if let Some(pos) = buf.find("ACTIVE=") {
        if let Ok(load) = buf[pos + 7..].trim().parse::<i32>() {
            debug!("mtk_mali {load}");
            return Ok(if load == 0 { mali_load()? } else { load });
        }
    }

    mali_load()
}

fn gpufreq_load() -> Result<i32> {
    if !get_status(GPU_FREQ_LOAD_PATH) {
        return mtk_load();
    }

    let file = match File::open(GPU_FREQ_LOAD_PATH) {
        Ok(file) => file,
        Err(_) => {
            write_status(GPU_FREQ_LOAD_PATH, false);
            return Ok(0);
        }
    };

    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line?;

        // Parse "gpu_loading = XX" format
        if let Some(pos) = line.find("gpu_loading = ") {
            if let Ok(load) = line[pos + 14..].trim().parse::<i32>() {
                debug!("gpufreq {load}");
                return Ok(if load == 0 { mtk_load()? } else { load });
            }
        }
    }

    mtk_load()
}

fn debug_dvfs_load_func() -> Result<i32> {
    // Check if debug_dvfs_load or debug_dvfs_load_old exists
    let path = if get_status(DEBUG_DVFS_LOAD) {
        DEBUG_DVFS_LOAD
    } else if get_status(DEBUG_DVFS_LOAD_OLD) {
        DEBUG_DVFS_LOAD_OLD
    } else {
        return gpufreq_load();
    };

    let buf = read_file(path, 256)?;
    let lines: Vec<&str> = buf.lines().collect();

    if lines.len() < 2 {
        return gpufreq_load();
    }

    // Static variables to keep track of previous values
    static mut PREV_BUSY: i64 = 0;
    static mut PREV_IDLE: i64 = 0;
    static mut PREV_PROTM: i64 = 0;

    // Parse the second line which contains the values
    let parts: Vec<&str> = lines[1].split_whitespace().collect();

    if parts.len() >= 3 {
        if let (Ok(busy), Ok(idle), Ok(protm)) = (
            parts[0].parse::<i64>(),
            parts[1].parse::<i64>(),
            parts[2].parse::<i64>(),
        ) {
            // Get previous values safely
            let (prev_busy, prev_idle, prev_protm) = unsafe { (PREV_BUSY, PREV_IDLE, PREV_PROTM) };

            // Calculate differences
            let diff_busy = busy - prev_busy;
            let diff_idle = idle - prev_idle;
            let diff_protm = protm - prev_protm;

            // Update previous values
            unsafe {
                PREV_BUSY = busy;
                PREV_IDLE = idle;
                PREV_PROTM = protm;
            }

            // Calculate load percentage
            let total = diff_busy + diff_idle + diff_protm;
            if total > 0 {
                let load = ((diff_busy + diff_protm) * 100 / total) as i32;
                let load = if load < 0 { 0 } else { load };

                debug!("debugutil: {load} {diff_busy} {diff_idle} {diff_protm}");
                return Ok(if load == 0 { mtk_load()? } else { load });
            }
        }
    }

    gpufreq_load()
}

pub fn get_gpu_load() -> Result<i32> {
    debug_dvfs_load_func()
}

pub fn get_gpu_current_freq(is_v1_driver: bool) -> Result<i64> {
    // 对于v1驱动设备，只使用gpufreq_var_dump方法读取频率
    if is_v1_driver {
        return read_v1_gpu_freq_from_var_dump();
    }

    // 对于v2驱动设备，使用原有的多路径读取策略
    // 首先尝试从GPU_CURRENT_FREQ_PATH读取频率
    if get_status(GPU_CURRENT_FREQ_PATH) {
        let buf = match read_file(GPU_CURRENT_FREQ_PATH, 64) {
            Ok(content) => content,
            Err(e) => {
                debug!("Failed to read GPU_CURRENT_FREQ_PATH: {e}");
                write_status(GPU_CURRENT_FREQ_PATH, false);
                // 不立即返回，继续尝试其他路径
                String::new()
            }
        };

        if !buf.is_empty() {
            let parts: Vec<&str> = buf.split_whitespace().collect();

            // 读取第二个整数作为当前频率
            if parts.len() >= 2 {
                if let Ok(freq) = parts[1].parse::<i64>() {
                    debug!("Current GPU frequency from {GPU_CURRENT_FREQ_PATH}: {freq}");
                    return Ok(freq);
                } else {
                    debug!("Failed to parse second value as frequency from: {buf}");
                }
            } else {
                debug!("Not enough values in GPU frequency file, content: {buf}");
            }
        }
    } else {
        debug!("GPU current frequency path not available: {GPU_CURRENT_FREQ_PATH}");
    }

    // 如果无法从GPU_CURRENT_FREQ_PATH读取，尝试从GPU_DEBUG_CURRENT_FREQ_PATH读取
    if get_status(GPU_DEBUG_CURRENT_FREQ_PATH) {
        let buf = match read_file(GPU_DEBUG_CURRENT_FREQ_PATH, 64) {
            Ok(content) => content,
            Err(e) => {
                debug!("Failed to read GPU_DEBUG_CURRENT_FREQ_PATH: {e}");
                write_status(GPU_DEBUG_CURRENT_FREQ_PATH, false);
                // 不立即返回，继续尝试其他路径
                String::new()
            }
        };

        if !buf.is_empty() {
            let parts: Vec<&str> = buf.split_whitespace().collect();

            // 读取第二个整数作为当前频率
            if parts.len() >= 2 {
                if let Ok(freq) = parts[1].parse::<i64>() {
                    debug!("Current GPU frequency from {GPU_DEBUG_CURRENT_FREQ_PATH}: {freq}");
                    return Ok(freq);
                } else {
                    debug!("Failed to parse second value as frequency from: {buf}");
                }
            } else {
                debug!("Not enough values in GPU frequency file, content: {buf}");
            }
        }
    } else {
        debug!("GPU debug current frequency path not available: {GPU_DEBUG_CURRENT_FREQ_PATH}");
    }

    // 如果无法从前两个路径读取，尝试从GPU_FREQ_LOAD_PATH读取（作为v2驱动的备用方案）
    read_v1_gpu_freq_from_var_dump()
}

/// 专门用于v1驱动设备的GPU频率读取函数
/// 只从/proc/gpufreq/gpufreq_var_dump文件读取频率
fn read_v1_gpu_freq_from_var_dump() -> Result<i64> {
    if !get_status(GPU_FREQ_LOAD_PATH) {
        return Err(anyhow!(
            "V1 driver frequency path not available: {GPU_FREQ_LOAD_PATH}"
        ));
    }

    debug!("Reading V1 driver GPU frequency from {GPU_FREQ_LOAD_PATH}");

    let file = match File::open(GPU_FREQ_LOAD_PATH) {
        Ok(file) => file,
        Err(e) => {
            debug!("Failed to open GPU_FREQ_LOAD_PATH: {e}");
            write_status(GPU_FREQ_LOAD_PATH, false);
            return Err(anyhow!(
                "Cannot read V1 driver GPU frequency: file open failed"
            ));
        }
    };

    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                debug!("Error reading line from GPU_FREQ_LOAD_PATH: {e}");
                continue;
            }
        };

        // 跳过长度小于等于3的行
        if line.len() <= 3 {
            continue;
        }

        // 尝试解析v1驱动的两种格式
        // 格式1：idx: [数字], freq: [频率], vgpu: [电压], vsram_gpu: [电压]
        if line.contains("idx:") && line.contains("freq:") {
            if let Some(freq_pos) = line.find("freq:") {
                let freq_part = &line[freq_pos + 5..];
                if let Some(comma_pos) = freq_part.find(',') {
                    let freq_str = freq_part[..comma_pos].trim();
                    if let Ok(freq) = freq_str.parse::<i64>() {
                        debug!(
                            "V1 driver GPU frequency from {GPU_FREQ_LOAD_PATH} (format 1): {freq}"
                        );
                        return Ok(freq);
                    }
                }
            }
        }
        // 格式2：Freq: [频率], Vgpu: [电压], Vsram_gpu: [电压]
        else if line.starts_with("Freq:") {
            if let Some(comma_pos) = line.find(',') {
                let freq_str = line[5..comma_pos].trim();
                if let Ok(freq) = freq_str.parse::<i64>() {
                    debug!("V1 driver GPU frequency from {GPU_FREQ_LOAD_PATH} (format 2): {freq}");
                    return Ok(freq);
                }
            }
        }
        // 兼容原有的"cur_freq = XX"格式（备用）
        else if let Some(pos) = line.find("cur_freq = ") {
            if let Ok(freq) = line[pos + 11..].trim().parse::<i64>() {
                debug!("V1 driver GPU frequency from {GPU_FREQ_LOAD_PATH} (legacy format): {freq}");
                return Ok(freq);
            }
        }
    }

    // 如果无法解析任何有效频率
    Err(anyhow!(
        "Cannot parse V1 driver GPU frequency from {GPU_FREQ_LOAD_PATH}"
    ))
}

pub fn utilization_init() -> Result<()> {
    let mut is_good = false;
    let mut freq_path_available = false;
    info!("Init LoadMonitor");
    info!("Testing GED...");

    // 方法1：从 /sys/module/ged 读取
    let module_load_status = check_read(MODULE_LOAD, &mut is_good);
    info!("{MODULE_LOAD}: {module_load_status}");
    let module_idle_status = check_read(MODULE_IDLE, &mut is_good);
    info!("{MODULE_IDLE}: {module_idle_status}");

    // 方法2：从 /sys/kernel/ged 读取
    let kernel_load_status = check_read(KERNEL_LOAD, &mut is_good);
    info!("{KERNEL_LOAD}: {kernel_load_status}");

    // 方法3：从 /sys/kernel/debug/ged 读取
    let kernel_debug_load_status = check_read(KERNEL_DEBUG_LOAD, &mut is_good);
    info!("{KERNEL_DEBUG_LOAD}: {kernel_debug_load_status}");
    let kernel_d_load_status = check_read(KERNEL_D_LOAD, &mut is_good);
    info!("{KERNEL_D_LOAD}: {kernel_d_load_status}");

    // 检查GPU频率路径
    info!("Testing GPU frequency paths...");
    let current_freq_status = check_read(GPU_CURRENT_FREQ_PATH, &mut freq_path_available);
    info!("{GPU_CURRENT_FREQ_PATH}: {current_freq_status}");

    // 检查GPU调试频率路径
    let debug_current_freq_status =
        check_read(GPU_DEBUG_CURRENT_FREQ_PATH, &mut freq_path_available);
    info!("{GPU_DEBUG_CURRENT_FREQ_PATH}: {debug_current_freq_status}");

    // 方法4：从 /proc/gpufreq 读取
    info!("Testing gpufreq Driver...");
    let freq_load_status = check_read(GPU_FREQ_LOAD_PATH, &mut freq_path_available);
    info!("{GPU_FREQ_LOAD_PATH}: {freq_load_status}");

    // 方法5：从Mali驱动读取
    info!("Testing mali driver...");
    let proc_mtk_load_status = check_read(PROC_MTK_LOAD, &mut is_good);
    info!("{PROC_MTK_LOAD}: {proc_mtk_load_status}");
    let proc_mali_load_status = check_read(PROC_MALI_LOAD, &mut is_good);
    info!("{PROC_MALI_LOAD}: {proc_mali_load_status}");

    // Method 6: Read precise load from Mali Driver
    let debug_dvfs_load_status = check_read(DEBUG_DVFS_LOAD, &mut is_good);
    info!("{DEBUG_DVFS_LOAD}: {debug_dvfs_load_status}");
    let debug_dvfs_load_old_status = check_read(DEBUG_DVFS_LOAD_OLD, &mut is_good);
    info!("{DEBUG_DVFS_LOAD_OLD}: {debug_dvfs_load_old_status}");

    // 检查是否可以监控GPU负载
    if !is_good {
        error!("Can't Monitor GPU Loading!");
        return Err(anyhow!("Can't Monitor GPU Loading!"));
    }

    // 检查是否可以读取GPU频率
    if !freq_path_available {
        error!("Can't read GPU frequency: all paths ({GPU_CURRENT_FREQ_PATH}, {GPU_DEBUG_CURRENT_FREQ_PATH}, {GPU_FREQ_LOAD_PATH}) are unavailable!");
        return Err(anyhow!(
            "Can't read GPU frequency: no valid frequency path available"
        ));
    }

    info!("Test Finished.");
    Ok(())
}
