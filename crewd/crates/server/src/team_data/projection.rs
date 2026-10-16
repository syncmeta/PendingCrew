//! ACL filtering of every typed pointer in a visible object. Protected capture
//! facts never leave this boundary, and a missing foreign capture never reads HEAD.
use crewd_data_protocol::Capture;
use crewd_proto::team_data::*;
use std::collections::{BTreeMap,BTreeSet};

pub struct Resolved { pub commit:Option<String>,pub revision:Option<String>,pub complete:bool }
pub trait Resolver {
    /// Implementations must recheck the original principal's CURRENT target ACL.
    /// None deliberately conflates denied, missing, and unavailable targets.
    /// Core links use their existing authorized read/navigation, never Dolt SQL.
    fn resolve(&self,reference:&DataRef,at:Option<&str>)->Result<Option<Resolved>,String>;
}
struct Projection<'a>{namespace:&'a str,commit:&'a str,historical:bool,captures:BTreeMap<&'a str,&'a Capture>,resolver:&'a dyn Resolver,references:Vec<DataResolvedRef>}
impl Projection<'_>{
 fn pointer(&mut self,anchor:&str,state:&mut DataRefState,pin:&mut Option<String>)->Result<(),String>{
  let DataRefState::Visible{reference}=state else{
   *pin=None;self.references.push(DataResolvedRef{anchor:anchor.into(),target:DataRefState::Unavailable,commit:None,revision:None,live:false,complete:false});return Ok(())
  };
  let original=reference.clone();let data=["block","view","relation"].contains(&original.kind.as_str());let live=pin.is_none();
  let capture=self.captures.get(anchor).copied();
  if let Some(c)=capture{if c.namespace!=original.namespace||c.kind!=original.kind||c.id!=original.id||c.live!=live{return Err("protected reference capture attribution mismatch".into())}}
  let exact=if data{
   if let Some(pin)=pin.as_ref(){Some(pin.clone())}
   else if self.historical&&original.namespace==self.namespace{Some(self.commit.into())}
   else if self.historical{match capture{Some(c)if c.commit!="self"=>Some(c.commit.clone()),_=>{
    *state=DataRefState::Unavailable;*pin=None;self.references.push(DataResolvedRef{anchor:anchor.into(),target:DataRefState::Unavailable,commit:None,revision:None,live,complete:false});return Ok(())
   }}}
   else{None}
  }else{if self.historical||pin.is_some(){
   *state=DataRefState::Unavailable;*pin=None;self.references.push(DataResolvedRef{anchor:anchor.into(),target:DataRefState::Unavailable,commit:None,revision:None,live,complete:false});return Ok(())
  }None};
  let resolved=self.resolver.resolve(&original,exact.as_deref())?;
  match resolved{
   Some(r)=>{
    if data&&(r.commit.is_none()||r.revision.is_none()||!r.complete){return Err("visible Dolt reference missing exact resolution".into())}
    if let (Some(expected),Some(actual))=(exact.as_ref(),r.commit.as_ref()){if expected!=actual{return Err("target read escaped exact snapshot".into())}}
    self.references.push(DataResolvedRef{anchor:anchor.into(),target:state.clone(),commit:r.commit,revision:r.revision,live,complete:r.complete});
   },
   None=>{*state=DataRefState::Unavailable;*pin=None;self.references.push(DataResolvedRef{anchor:anchor.into(),target:DataRefState::Unavailable,commit:None,revision:None,live,complete:false});}
  }
  Ok(())
 }
 fn provenance(&mut self,p:&mut DataProvenance)->Result<(),String>{
  for (index,source) in p.sources.iter_mut().enumerate(){if let Some(reference)=&mut source.reference{self.pointer(&format!("source:{index}"),reference,&mut source.at_commit)?;}}
  Ok(())
 }
}
/// Caller already authorized the parent object at the exact `commit`. This
/// function does not mutate the protected stored object or its capture vector.
pub fn project(mut object:DataObject,captures:&[Capture],namespace:&str,commit:&str,historical:bool,resolver:&dyn Resolver)->Result<(DataObject,Vec<DataResolvedRef>),String>{
 let mut names=BTreeSet::new();for c in captures{if !names.insert(c.anchor.as_str()){return Err("duplicate protected capture anchor".into())}}
 let mut p=Projection{namespace,commit,historical,captures:captures.iter().map(|c|(c.anchor.as_str(),c)).collect(),resolver,references:vec![]};
 match &mut object{
  DataObject::Block{provenance,..}=>p.provenance(provenance)?,
  DataObject::View{definition,provenance,..}=>{
   match definition{
    DataViewDefinition::Document{placements,..}=>for placement in placements{p.pointer(&format!("placement:{}",placement.id),&mut placement.block,&mut placement.at_commit)?;},
    DataViewDefinition::Web{bindings,..}=>for binding in bindings{if let DataBindingSource::Object{reference,at_commit}=&mut binding.source{p.pointer(&format!("binding:{}",binding.id),reference,at_commit)?;}},
   }
   p.provenance(provenance)?;
  },
  DataObject::Relation{subject,object,provenance,..}=>{
   let mut no_pin=None;p.pointer("relation:subject",subject,&mut no_pin)?;let mut no_pin=None;p.pointer("relation:object",object,&mut no_pin)?;p.provenance(provenance)?;
  }
 }
 Ok((object,p.references))
}
