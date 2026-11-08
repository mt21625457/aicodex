use super::Gauge;
use super::snapshot;
use pretty_assertions::assert_eq;
use std::sync::atomic::Ordering;

static GUARD_GAUGE: Gauge = Gauge::new("diagnostics.tests.guard");
static REGISTERED_GAUGE: Gauge = Gauge::new("diagnostics.tests.registered");

#[test]
fn decrement_at_zero_does_not_underflow_and_can_resume_counting() {
    // 局部计数器不加入全局注册表，避免与其他测试共享状态。
    let gauge = Gauge::new("diagnostics.tests.zero");
    gauge.decrement();
    gauge.decrement();
    assert_eq!(gauge.value.load(Ordering::Relaxed), 0);

    gauge.value.store(2, Ordering::Relaxed);
    gauge.decrement();
    assert_eq!(gauge.value.load(Ordering::Relaxed), 1);
    gauge.decrement();
    gauge.decrement();
    assert_eq!(gauge.value.load(Ordering::Relaxed), 0);
}

#[test]
fn concurrent_decrements_preserve_updates_and_stop_at_zero() {
    let gauge = Gauge::new("diagnostics.tests.concurrent");
    gauge.value.store(1_000, Ordering::Relaxed);
    // 屏障让四个线程竞争同一计数器；作用域结束后再检查，避免依赖时间延迟。
    for expected in [200, 0] {
        let barrier = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let gauge = &gauge;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    for _ in 0..200 {
                        gauge.decrement();
                    }
                });
            }
        });
        assert_eq!(gauge.value.load(Ordering::Relaxed), expected);
    }
}

#[test]
fn gauge_guards_follow_the_measured_lifetime() {
    let first = GUARD_GAUGE.track();
    let second = GUARD_GAUGE.track();
    let value = || {
        snapshot()
            .gauges
            .into_iter()
            .find(|gauge| gauge.name == "diagnostics.tests.guard")
            .expect("used gauge should be registered")
            .value
    };

    assert_eq!(value(), 2);
    drop(first);
    assert_eq!(value(), 1);
    drop(second);
    assert_eq!(value(), 0);
}

#[test]
fn snapshot_includes_process_memory_and_registers_gauges_once() {
    REGISTERED_GAUGE.increment();
    REGISTERED_GAUGE.increment();
    let diagnostics = snapshot();
    let registered = diagnostics
        .gauges
        .iter()
        .filter(|gauge| gauge.name == "diagnostics.tests.registered")
        .collect::<Vec<_>>();

    assert_eq!(diagnostics.process.id, std::process::id());
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0].value, 2);
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    assert!(
        diagnostics
            .process
            .resident_memory_bytes
            .is_some_and(|bytes| bytes > 0)
    );
    #[cfg(target_os = "macos")]
    assert!(
        diagnostics
            .process
            .physical_footprint_bytes
            .is_some_and(|bytes| bytes > 0)
    );
    #[cfg(not(target_os = "macos"))]
    assert_eq!(diagnostics.process.physical_footprint_bytes, None);
}
