//! Stable-ID semantic import planning. The plan contains IDs/ranges/source
//! witnesses, never a second body store in the Core control ledger.
use crewd_proto::team_data::*;
use crewd_store::team_data_legacy::{LegacyDocument,SourceWitness};
use crewd_data_protocol::{Mutation,Capture};
use serde::{Serialize,Deserialize};
use serde_json::{Value,json};

#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct BlockPlan{pub id:String,pub placement_id:String,pub start_byte:usize,pub end_byte:usize,pub semantic_kind:String}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct DocumentPlan{pub source:SourceWitness,pub view_id:String,pub blocks:Vec<BlockPlan>}
fn uuid()->String{uuid::Uuid::new_v4().to_string()}
fn fence(line:&str)->Option<(u8,usize)>{
 let line=line.trim_end_matches(['\r','\n']);let trimmed=line.trim_start_matches(' ');if line.len()-trimmed.len()>3{return None}let first=*trimmed.as_bytes().first()?;if first!=b'`'&&first!=b'~'{return None}let count=trimmed.bytes().take_while(|b|*b==first).count();(count>=3).then_some((first,count))
}
fn heading(line:&str)->bool{let line=line.trim_end_matches(['\r','\n']);let trimmed=line.trim_start_matches(' ');if line.len()-trimmed.len()>3{return false}let count=trimmed.bytes().take_while(|b|*b==b'#').count();(1..=6).contains(&count)&&trimmed.as_bytes().get(count).is_none_or(|b|b.is_ascii_whitespace())}
/// Preserve all separators byte-for-byte. Ambiguous/unclosed fences and very
/// fragmented inputs stay one whole semantic block; there is no token slicing.
pub fn semantic_ranges(body:&str)->Vec<(usize,usize,String)>{
 if body.is_empty(){return vec![]}
 let mut ranges=Vec::new();let mut start=0;let mut offset=0;let mut blank_boundary=false;let mut active_fence:Option<(u8,usize)>=None;let mut kind="paragraph";
 for line in body.split_inclusive('\n'){
  let end=offset+line.len();let blank=line.trim().is_empty();
  if let Some((marker,count))=active_fence{
   if let Some((close,run))=fence(line){let tail=line.trim_start_matches(' ').trim_end_matches(['\r','\n']);if close==marker&&run>=count&&tail[run..].trim().is_empty(){active_fence=None;blank_boundary=true;}}
  }else{
   let opening=fence(line);let is_heading=heading(line);
   if !blank&&offset>start&&!body[start..offset].trim().is_empty()&&(blank_boundary||opening.is_some()||is_heading){ranges.push((start,offset,kind.to_owned()));start=offset;kind="paragraph";}
   if let Some(f)=opening{active_fence=Some(f);kind="codeFence";}
   else if is_heading{kind="heading";blank_boundary=true;offset=end;continue}
   blank_boundary=blank;
  }
  offset=end;
 }
 if active_fence.is_some(){return vec![(0,body.len(),"whole".into())]}
 if start<body.len(){ranges.push((start,body.len(),kind.into()))}
 if ranges.len()>4096{return vec![(0,body.len(),"whole".into())]}
 ranges
}
pub fn prepare(document:&LegacyDocument)->Result<DocumentPlan,String>{
 uuid::Uuid::parse_str(&document.summary.id).map_err(|_|"legacy document ID is not a stable UUID")?;
 let source=document.witness().map_err(|_|"legacy source witness invalid")?;
 let blocks=semantic_ranges(&document.body).into_iter().map(|(start_byte,end_byte,semantic_kind)|BlockPlan{id:uuid(),placement_id:uuid(),start_byte,end_byte,semantic_kind}).collect();
 Ok(DocumentPlan{source,view_id:document.summary.id.clone(),blocks})
}
/// Materialize only after resume revalidates the COMPLETE source witness.
/// Imported existing revisions are deliberately not sent through ordinary
/// create-with-revision1 Apply; the owning import controller uses a protected
/// typed import operation before format16 activation.
pub fn materialize(document:&LegacyDocument,plan:&DocumentPlan,namespace:&str)->Result<Vec<Mutation>,String>{
 if plan.view_id!=document.summary.id||plan.source!=document.witness().map_err(|_|"legacy source corrupt")?{return Err("legacy source changed after ID allocation".into())}
 let mut ids=std::collections::BTreeSet::from([plan.view_id.clone()]);let mut end=0;let mut placements=Vec::new();let mut mutations=Vec::new();let m=&document.summary;
 for block in &plan.blocks{
  if block.start_byte!=end||block.end_byte<=block.start_byte||!document.body.is_char_boundary(block.start_byte)||!document.body.is_char_boundary(block.end_byte)||block.end_byte>document.body.len()||!ids.insert(block.id.clone())||!ids.insert(block.placement_id.clone()){return Err("invalid preserved semantic range/ID".into())}
  for id in [&block.id,&block.placement_id]{if uuid::Uuid::parse_str(id).is_err(){return Err("invalid planned stable UUID".into())}}
  end=block.end_byte;let source=DataRef{namespace:namespace.into(),kind:"view".into(),id:plan.view_id.clone()};
  let provenance=DataProvenance{created_by:m.updated_by.clone(),updated_by:m.updated_by.clone(),created_at:m.updated_at.clone(),updated_at:m.updated_at.clone(),sources:vec![DataSource{reference:Some(DataRefState::Visible{reference:source.clone()}),at_commit:None,attributes:json!({"derivation":"legacyCurrentText","sourceRevision":m.revision})}],attributes:json!({"semanticKind":block.semantic_kind,"legacyByteRange":[block.start_byte,block.end_byte]})};
  let object=DataObject::Block{id:block.id.clone(),revision:"1".into(),content_type:"text/plain".into(),content:Value::String(document.body[block.start_byte..block.end_byte].into()),archived:false,provenance};
  mutations.push(Mutation::PutObject{object,expected_revision:None,captures:vec![Capture{anchor:"source:0".into(),namespace:namespace.into(),kind:"view".into(),id:plan.view_id.clone(),commit:"self".into(),revision:m.revision.clone(),live:true}]});
  placements.push(DataPlacement{id:block.placement_id.clone(),block:DataRefState::Visible{reference:DataRef{namespace:namespace.into(),kind:"block".into(),id:block.id.clone()}},at_commit:None,layout:json!({})});
 }
 if end!=document.body.len(){return Err("semantic ranges do not preserve complete document".into())}
 let provenance=DataProvenance{created_by:m.created_by.clone(),updated_by:m.updated_by.clone(),created_at:m.created_at.clone(),updated_at:m.updated_at.clone(),sources:vec![],attributes:json!({"importedLegacyRevision":m.revision,"legacySourceSHA256":plan.source.complete_source_sha256})};
 let captures=placements.iter().map(|p|Capture{anchor:format!("placement:{}",p.id),namespace:namespace.into(),kind:"block".into(),id:match &p.block{DataRefState::Visible{reference}=>reference.id.clone(),_=>unreachable!()},commit:"self".into(),revision:"1".into(),live:true}).collect();
 mutations.push(Mutation::PutObject{object:DataObject::View{id:plan.view_id.clone(),revision:m.revision.clone(),title:m.title.clone(),definition:DataViewDefinition::Document{placements,layout:json!({})},archived:m.deleted_at.is_some(),provenance},expected_revision:None,captures});Ok(mutations)
}
#[cfg(test)]mod tests{
 use super::*;
 #[test]fn semantic_cut_preserves_unicode_crlf_and_does_not_slice_unclosed_fences(){
  for body in ["","\r\n  leading\r\n\r\n# 标题\r\nfirst paragraph\r\n\r\n```rust\r\nlet x = 1;\r\n```\r\nnext paragraph\r\n","same\n\nsame\n", "  ```\nno closing marker", "   "]{
   let spans=semantic_ranges(body);let joined=spans.iter().map(|(s,e,_)|&body[*s..*e]).collect::<String>();assert_eq!(joined,body);assert!(spans.iter().all(|(s,e,_)|body.is_char_boundary(*s)&&body.is_char_boundary(*e)));
  }
  assert_eq!(semantic_ranges("```\nunknown").as_slice(),&[(0,11,"whole".into())]);
 }
}
