use super::*;

#[test]
fn missing_or_invalid_host_readings_are_not_quiet() {
    let psi = "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
    let quiet = HostLoad::from_readings(Some("0.1 0.2 0.3 1/2 3"), Some(16), Some(psi), Some(psi));
    assert!(quiet.quiet());
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
        assert!(!unknown.quiet(), "{unknown:?}");
        let json = unknown.to_json();
        let encoded = serde_json::to_string(&json).unwrap();
        let decoded: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, json);
        assert_eq!(decoded["quiet"], false);
        assert!(decoded.as_object().unwrap().values().any(Value::is_null));
    }
}

fn quiet() -> HostLoad {
    HostLoad {
        load1: Some(0.1),
        cpus: Some(16),
        io_full: Some(0.0),
        memory_full: Some(0.0),
    }
}

#[test]
fn busy_or_unknown_middle_samples_survive_quiet_endpoints() {
    for middle in [
        HostLoad {
            io_full: Some(50.0),
            ..quiet()
        },
        HostLoad {
            io_full: None,
            ..quiet()
        },
    ] {
        let mut interval = LoadInterval::new(quiet());
        interval.observe(Duration::from_secs(1), middle);
        interval.observe(Duration::from_secs(2), quiet());
        assert!(!interval.checks_passed());
        let value = interval.to_json();
        assert_eq!(value["start"]["quiet"], true);
        assert_eq!(value["end"]["quiet"], true);
        assert_eq!(value["sample_count"], 3);
    }
}

#[test]
fn long_sampling_gaps_cannot_pass() {
    let mut interval = LoadInterval::new(quiet());
    interval.observe(Duration::from_secs(5), quiet());
    assert!(!interval.checks_passed());
    assert_eq!(interval.to_json()["max_gap_seconds"], 5.0);
    let mut valid = LoadInterval::new(quiet());
    valid.observe(Duration::from_secs(1), quiet());
    assert!(valid.checks_passed());
}

#[test]
fn sampler_finishes_with_start_and_end_observations() {
    let interval = LoadMonitor::start().unwrap().finish().unwrap();
    assert!(interval.samples >= 2);
    assert!(interval.elapsed.as_secs_f64().is_finite());
}
