use crewd_exec::codex::{control::{CodexOperation, CodexOutcome}, metadata::{run,MetadataRequest}, transport::CodexProcessSpec};
#[tokio::test]
async fn quota_metadata_only_protocol_and_rejection() {
    for reject in [false,true] {
        let dir=tempfile::tempdir().unwrap();
        let script=dir.path().join("fake.py");
        std::fs::write(&script, format!(r#"
import sys,json
for line in sys.stdin:
 r=json.loads(line)
 m=r['method']
 assert m in ['initialize','initialized','account/rateLimits/read'], m
 if m=='initialize': print(json.dumps({{'id':r['id'],'result':{{}}}}),flush=True)
 if m=='account/rateLimits/read':
  assert r['params']=={{}}
  result={{'error':{{'code':-32601,'message':'unsupported'}}}} if {reject} else {{'result':{{'rateLimits':{{'primary':{{'usedPercent':12}}}},'rateLimitsByLimitId':{{}}}}}}
  print(json.dumps(dict(id=r['id'],**result)),flush=True)
"#,reject=if reject {"True"} else {"False"})).unwrap();
        let result=run(CodexProcessSpec{program:"/usr/bin/python3".into(),args:vec![script.to_string_lossy().into()],env:Default::default(),cwd:dir.path().into()},None,MetadataRequest{session_id:"host-quota".into(),thread_id:String::new(),workdir:dir.path().into(),operation:CodexOperation::RateLimits,permit:None}).await;
        if reject { assert!(matches!(result,CodexOutcome::Rejected(_))); }
        else { let CodexOutcome::Observed(v)=result else {panic!("{result:?}")}; assert_eq!(v["providerResult"]["rateLimits"]["primary"]["usedPercent"],12); }
    }
}
