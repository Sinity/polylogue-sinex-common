use super::*;
fn store()->Store {
 let s=Store::open_at(std::path::Path::new(":memory:")).unwrap();
 for (id,ms) in [(1,100),(2,200)] { s.conn.execute("INSERT INTO runs(id,project,checkout,command,args,host,toolchain,pid,started_ms,duration_ms,status) VALUES(?1,'demo','/repo','test','[]','host','rust',0,?1*50,?2,'passed')",params![id,ms]).unwrap(); }
 s
}
#[test] fn comparisons_preserve_scope_and_unknowns() {
 let s=store();assert_eq!(s.compare("demo",1,2).unwrap()["duration_change"]["trend"],"slower");
 assert_eq!(s.compare("demo",2,1).unwrap()["duration_change"]["trend"],"faster");
 assert_eq!(s.compare("demo",1,1).unwrap()["duration_change"]["trend"],"stable");
 assert!(s.compare("other",1,2).is_err());
 for field in ["args","host","toolchain","status"] {
  let original:String=s.conn.query_row(&format!("SELECT {field} FROM runs WHERE id=2"),[],|r|r.get(0)).unwrap();
  s.conn.execute(&format!("UPDATE runs SET {field}='different' WHERE id=2"),[]).unwrap();assert!(s.compare("demo",1,2).is_err());
  s.conn.execute(&format!("UPDATE runs SET {field}=?1 WHERE id=2"),[original]).unwrap();
 }
 s.conn.execute("UPDATE runs SET duration_ms=NULL WHERE id=2",[]).unwrap();assert!(s.compare("demo",1,2).unwrap()["duration_change"].is_null());
}
#[test] fn pass_rates_stages_and_overlap_do_not_sum_parallel_test_time() {
 let s=store();s.conn.execute_batch("INSERT INTO tests VALUES(1,'suite','a','passed',90,NULL),(1,'suite','b','failed',80,NULL),(1,'suite','c','filtered',0,NULL);INSERT INTO stages VALUES(1,0,'database',10,1),(1,1,'test',90,1);").unwrap();
 let report=s.explain("demo",1).unwrap();assert_eq!(report["pass_rate_percent"],50.0);assert_eq!(report["stages"][0]["name"],"database");assert_eq!(report["overlaps"][0]["overlap_ms"],50);assert!(report["queue_ms"].is_null());assert!(report["resources"].is_null());
 assert!(s.explain("demo",2).unwrap()["pass_rate_percent"].is_null());
}
#[test] fn daily_comparisons_keep_exact_scope_and_validate_dates() {
 let s=store();assert!(s.compare_days("demo","/repo","yesterday","2026-10-27").is_err());
 s.conn.execute("UPDATE runs SET started_ms=started_ms+86400000 WHERE id=2",[]).unwrap();
 assert_eq!(s.compare_days("demo","/repo","1970-01-01","1970-01-02").unwrap()["changes"][0]["change"]["trend"],"slower");
 s.conn.execute(r#"UPDATE runs SET args='["-p","one"]' WHERE id=2"#,[]).unwrap();
 assert!(s.compare_days("demo","/repo","1970-01-01","1970-01-02").unwrap()["changes"].as_array().unwrap().is_empty());
}

#[test]
fn interrupted_runs_keep_unknown_duration_and_are_not_duration_baselines() {
    let s=store();s.conn.execute("UPDATE runs SET status='running',pid=-1,duration_ms=NULL WHERE id=2",[]).unwrap();
    let report=s.explain("demo",2).unwrap();assert_eq!(report["header"]["status"],"interrupted");assert!(report["header"]["duration_ms"].is_null());assert!(s.compare("demo",1,2).is_err());
}
