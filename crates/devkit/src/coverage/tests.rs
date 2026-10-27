use super::*;
#[test]
fn totals_are_weighted_and_missing_values_fail() {
    assert_eq!(percentage(&json!({"data":[{"totals":{"lines":{"count":10,"covered":5}}},{"totals":{"lines":{"count":90,"covered":90}}}]})).unwrap(),95.0);
    for report in [json!({}),json!({"data":[]}),json!({"data":[{"totals":{"lines":{"count":1,"covered":2}}}]}),json!({"data":[{"totals":{"lines":{"count":1}}}]})] { assert!(percentage(&report).is_err()); }
}
#[test]
fn regions_use_the_functions_own_file_indices() {
    let report=json!({"data":[{"functions":[{"filenames":["second.rs","first.rs"],"regions":[[1,2,3,4,5,1,0,0]]}]}]});
    let mapped=regions(&report).unwrap();assert_eq!(mapped[0]["file"],"first.rs");assert_eq!(mapped[0]["end_line"],3);
    assert!(regions(&json!({"data":[{"functions":[{"filenames":[],"regions":[[1,2,3,4,5,0]]}]}]})).is_err());
}
#[test]
fn reports_keep_package_selection_and_drop_test_filters() {
    let args=["-p","chosen","-E","test(example)","--all-features"].map(str::to_owned);
    assert_eq!(report_args(&args).unwrap(),["-p","chosen","--all-features"]);
}
#[test]
fn invalid_threshold_and_reused_output_fail_before_running() {
    let dir=tempfile::tempdir().unwrap();let mut run=Run::start("demo","coverage",&[]);
    let mut options=Coverage{output:dir.path().into(),format:"json".into(),threshold:Some(f64::NAN),args:vec![]};
    assert!(execute(&mut run,&options,&[]).unwrap_err().contains("threshold"));
    options.threshold=None;fs::write(dir.path().join("coverage.json"),"{}").unwrap();
    assert!(execute(&mut run,&options,&[]).unwrap_err().contains("already used"));
}

fn fake_report(script: &str, format: &str, threshold: Option<f64>) -> (tempfile::TempDir, Result<(),String>) {
    let dir=tempfile::tempdir().unwrap();
    let options=Coverage{output:dir.path().join("output"),format:format.into(),threshold,args:vec!["-p".into(),"selected".into(),"-E".into(),"test(chosen)".into()]};
    let mut run=Run::start("demo","coverage",&[]);
    let result=execute_with(&mut run,&options,&[],||{
        let mut command=std::process::Command::new("sh");command.args(["-c",script,"fake-cargo"]);command
    });
    (dir,result)
}
#[test]
fn failed_missing_and_malformed_reports_never_succeed() {
    assert!(fake_report("exit 9","json",None).1.unwrap_err().contains("exited"));
    assert!(fake_report("exit 0","json",None).1.unwrap_err().contains("coverage report"));
    let script=r#"while [ "$#" -gt 0 ]; do if [ "$1" = --output-path ]; then shift; printf '%s' invalid > "$1"; fi; shift; done"#;
    assert!(fake_report(script,"json",None).1.is_err());
}
#[test]
fn threshold_and_output_checks_use_actual_report() {
    let script=r#"while [ "$#" -gt 0 ]; do if [ "$1" = --output-path ]; then shift; printf '%s' '{"data":[{"totals":{"lines":{"count":10,"covered":5}},"functions":[{"filenames":["synthetic.rs"],"regions":[[1,1,10,1,5,0,0,0]]}]}]}' > "$1"; fi; shift; done"#;
    let (dir,result)=fake_report(script,"json",Some(40.0));assert!(result.is_ok());
    let summary:Value=serde_json::from_slice(&fs::read(dir.path().join("output/summary.json")).unwrap()).unwrap();assert_eq!(summary["args"][1],"selected");assert_eq!(summary["line_percent"],50.0);
    assert!(fake_report(script,"json",Some(60.0)).1.unwrap_err().contains("below"));
    assert!(fake_report(script,"html",None).1.unwrap_err().contains("report missing"));
}
