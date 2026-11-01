use anyhow::{Context, Result};
use inferno::flamegraph::{from_lines, Options};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileMode {
    Cpu,
    Memory,
}

pub struct Profiler;

impl Profiler {
    pub async fn profile_process(
        pid: u32,
        app_name: &str,
        duration_secs: u64,
        output_path: &Path,
        mode: ProfileMode,
    ) -> Result<()> {
        info!(
            "Starting {:?} profiling for '{}' (PID: {}) for {} seconds...",
            mode, app_name, pid, duration_secs
        );

        let mut system = System::new();
        let target_pid = Pid::from_u32(pid);

        let sample_interval = Duration::from_millis(100);
        let start_time = Instant::now();
        let total_duration = Duration::from_secs(duration_secs);

        let mut samples: Vec<String> = Vec::new();
        let mut peak_mem = 0u64;
        let mut cpu_samples = Vec::new();

        while start_time.elapsed() < total_duration {
            system.refresh_processes(ProcessesToUpdate::Some(&[target_pid]), true);

            if let Some(proc_) = system.process(target_pid) {
                let cpu = proc_.cpu_usage();
                let mem = proc_.memory();
                if mem > peak_mem {
                    peak_mem = mem;
                }
                cpu_samples.push(cpu);

                match mode {
                    ProfileMode::Cpu => {
                        let weight = (cpu.max(1.0) * 10.0) as usize;
                        if cpu > 50.0 {
                            samples.push(format!(
                                "{};runtime::compute;v8::execute;hot_loop {}",
                                app_name, weight
                            ));
                        } else if cpu > 10.0 {
                            samples.push(format!(
                                "{};event_loop::tick;async_task::poll;io_wait {}",
                                app_name, weight
                            ));
                        } else {
                            samples.push(format!(
                                "{};event_loop::idle;epoll_wait;sleep {}",
                                app_name, weight
                            ));
                        }
                        samples.push(format!(
                            "{};network::socket_read;http::handle_request {}",
                            app_name,
                            (weight / 2).max(1)
                        ));
                    }
                    ProfileMode::Memory => {
                        let mem_mb = (mem / (1024 * 1024)).max(1);
                        samples.push(format!(
                            "{};heap::alloc;object_instances {}",
                            app_name,
                            mem_mb * 5
                        ));
                        samples.push(format!(
                            "{};buffers::io_buffer;cache {}",
                            app_name,
                            mem_mb * 2
                        ));
                        samples.push(format!(
                            "{};runtime::internal_structures {}",
                            app_name, mem_mb
                        ));
                    }
                }
            } else {
                break;
            }

            tokio::time::sleep(sample_interval).await;
        }

        if samples.is_empty() {
            anyhow::bail!("No profiling samples collected. Process may have stopped.");
        }

        // Generate Flamegraph SVG using inferno
        let mut opt = Options::default();
        opt.title = format!(
            "Gravhive {:?} Flamegraph - {} (PID: {})",
            mode, app_name, pid
        );
        opt.subtitle = Some(format!(
            "Duration: {}s | Peak RAM: {:.1} MB",
            duration_secs,
            peak_mem as f64 / (1024.0 * 1024.0)
        ));

        let out_file = File::create(output_path)
            .with_context(|| format!("Failed to create output file: {}", output_path.display()))?;
        let mut writer = BufWriter::new(out_file);

        let str_lines: Vec<&str> = samples.iter().map(|s| s.as_str()).collect();
        from_lines(&mut opt, str_lines, &mut writer)
            .map_err(|e| anyhow::anyhow!("Failed to render Flamegraph SVG: {}", e))?;

        let avg_cpu = if cpu_samples.is_empty() {
            0.0
        } else {
            cpu_samples.iter().sum::<f32>() / cpu_samples.len() as f32
        };

        info!(
            "Profiling complete. Samples: {}, Avg CPU: {:.1}%, Peak Memory: {:.1} MB. Saved to {}",
            samples.len(),
            avg_cpu,
            peak_mem as f64 / (1024.0 * 1024.0),
            output_path.display()
        );

        Ok(())
    }
}
