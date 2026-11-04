//! 命令输出格式化。

use alloc::format;
use alloc::string::String;

use hk_root_server::{RuntimeStats, ServiceTable, TaskInfo, TraceEvent};

pub(crate) fn format_tasks(tasks: &[TaskInfo], current: Option<&str>) -> String {
    if tasks.is_empty() {
        return String::from("任务表为空\n");
    }

    let mut output = format!("任务表（{} 项）\n", tasks.len());
    output.push_str(
        "TCB  名称              父   端点  状态      挂起   退出码  句柄  Arena     VSpace       当前\n",
    );
    for task in tasks {
        let exit = match task.exit_code {
            Some(code) => format!("{code}"),
            None => String::from("-"),
        };
        let parent = match task.parent {
            Some(id) => format!("{id}"),
            None => String::from("-"),
        };
        let arena = format!("{}/{}K", task.arena_used / 1024, task.arena_size / 1024);
        let vspace = format!("{:#x}", task.vspace);
        let marker = if current == Some(task.name.as_str()) {
            "*"
        } else {
            ""
        };
        output.push_str(&format!(
            "{:<4} {:<16} {:<4} {:<4} {:<8} {:<5} {:<6} {:<4} {:<9} {:<12} {}\n",
            task.tcb,
            task.name,
            parent,
            task.endpoint,
            task.state,
            task.pending,
            exit,
            task.handles,
            arena,
            vspace,
            marker
        ));
    }
    output
}

pub(crate) fn format_handles(tasks: &[TaskInfo]) -> String {
    if tasks.is_empty() {
        return String::from("任务表为空\n");
    }

    let mut output = format!("Handle 计数（{} 项）\n", tasks.len());
    output.push_str("TCB  名称              句柄\n");
    for task in tasks {
        output.push_str(&format!(
            "{:<4} {:<16} {}\n",
            task.tcb, task.name, task.handles
        ));
    }
    output
}

pub(crate) fn format_trace(events: &[TraceEvent]) -> String {
    if events.is_empty() {
        return String::from("trace 为空\n");
    }

    let mut output = format!("trace（{} 项）\n", events.len());
    output.push_str("序号  类别   线程  代码  参数\n");
    for event in events {
        output.push_str(&format!(
            "{:<4} {:<6} {:<4} {:<4} {:#x}\n",
            event.sequence,
            event.category.name(),
            event.thread,
            event.code,
            event.argument
        ));
    }
    output
}

pub(crate) fn format_stats(stats: &RuntimeStats) -> String {
    let mut output = String::from("运行时统计\n");
    if stats.tsc_frequency == 0 {
        output.push_str("  TSC 频率：未知\n");
        return output;
    }

    output.push_str(&format!(
        "  上下文切换：{} 次，平均 {} ns，最大 {} ns\n",
        stats.switches,
        average_ns(stats.switch_cycles, stats.switches, stats.tsc_frequency),
        ns(stats.switch_max_cycles, stats.tsc_frequency),
    ));
    output.push_str(&format!(
        "  IPC call/reply：{} 次，平均 {} ns，最大 {} ns\n",
        stats.calls,
        average_ns(stats.call_cycles, stats.calls, stats.tsc_frequency),
        ns(stats.call_max_cycles, stats.tsc_frequency),
    ));
    let mut cpus = String::from("  CPU 调度：");
    for (cpu, count) in stats.resumes.iter().enumerate() {
        if *count > 0 {
            cpus.push_str(&format!("CPU{cpu}={count} "));
        }
    }
    cpus.push('\n');
    output.push_str(&cpus);
    output
}

fn average_ns(cycles: u64, count: u64, frequency: u64) -> u64 {
    if count == 0 || frequency == 0 {
        return 0;
    }
    ((u128::from(cycles) * 1_000_000_000) / (u128::from(frequency) * u128::from(count))) as u64
}

fn ns(cycles: u64, frequency: u64) -> u64 {
    if frequency == 0 {
        return 0;
    }
    ((u128::from(cycles) * 1_000_000_000) / u128::from(frequency)) as u64
}

pub(crate) fn format_services(services: &ServiceTable) -> String {
    if services.is_empty() {
        return String::from("服务表为空\n");
    }

    let mut output = format!("服务表（{} 项）\n", services.len());
    output.push_str("名称              状态      重启  退出码\n");
    for service in services.services() {
        let exit = match service.exit_code {
            Some(code) => format!("{code}"),
            None => String::from("-"),
        };
        output.push_str(&format!(
            "{:<16}  {:<8}  {:<4}  {}\n",
            service.name,
            service.state.name(),
            service.restarts,
            exit
        ));
    }
    output
}
