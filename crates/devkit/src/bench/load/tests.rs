use super::*;

#[test]
fn missing_or_invalid_host_readings_stay_unknown() {
    let psi = "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\nfull avg10=0.00 avg60=1.50 avg300=0.00 total=0\n";
    let known = HostLoad::from_readings(Some("0.1 0.2 0.3 1/2 3"), Some(16), Some(psi), Some(psi));
    assert_eq!(
        known,
        HostLoad {
            load1: Some(0.1),
            cpus: Some(16),
            io_full: Some(1.5),
            memory_full: Some(1.5),
        }
    );
    for unknown in [
        HostLoad::from_readings(None, Some(16), Some(psi), Some(psi)),
        HostLoad::from_readings(Some("0.1"), None, Some(psi), Some(psi)),
        HostLoad::from_readings(Some("0.1"), Some(16), None, Some(psi)),
        HostLoad::from_readings(Some("0.1"), Some(16), Some(psi), Some("some avg60=0")),
        HostLoad::from_readings(Some("NaN"), Some(16), Some(psi), Some(psi)),
        HostLoad::from_readings(Some("-1"), Some(16), Some(psi), Some(psi)),
        HostLoad::from_readings(Some("0.1"), Some(0), Some(psi), Some(psi)),
        HostLoad::from_readings(Some("0.1"), Some(16), Some("full avg60=inf"), Some(psi)),
    ] {
        let json = unknown.to_json();
        let encoded = serde_json::to_string(&json).unwrap();
        let decoded: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, json);
        assert!(decoded.as_object().unwrap().values().any(Value::is_null));
    }
}

fn idle() -> HostLoad {
    HostLoad {
        load1: Some(0.1),
        cpus: Some(16),
        io_full: Some(0.0),
        memory_full: Some(0.0),
    }
}

#[test]
fn a_busy_or_unknown_middle_sample_survives_in_the_summary() {
    let mut interval = LoadInterval::new(idle());
    interval.observe(
        Duration::from_secs(1),
        HostLoad {
            io_full: Some(50.0),
            ..idle()
        },
    );
    interval.observe(
        Duration::from_secs(2),
        HostLoad {
            io_full: None,
            ..idle()
        },
    );
    interval.observe(Duration::from_secs(7), idle());
    let value = interval.to_json();
    assert_eq!(value["sample_count"], 4);
    assert_eq!(value["unknown_samples"], 1);
    assert_eq!(value["peak"]["io_full"], 50.0);
    assert_eq!(value["start"]["io_full"], 0.0);
    assert_eq!(value["end"]["io_full"], 0.0);
    assert_eq!(value["max_gap_seconds"], 5.0);
}

#[test]
fn sampler_finishes_with_start_and_end_observations() {
    let interval = LoadMonitor::start().unwrap().finish().unwrap();
    assert!(interval.samples >= 2);
    assert!(interval.elapsed.as_secs_f64().is_finite());
}
