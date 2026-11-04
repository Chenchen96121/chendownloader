import {useEffect,useState} from "react";
import {getTikTokRuntimeStatus,type TikTokRuntimeStatus} from "../../shared/ipc/tiktok";
import type {Catalog} from "../../shared/i18n/catalog";

export function useTikTokComponent(active=true){
 const [status,setStatus]=useState<TikTokRuntimeStatus|null>(null);
 useEffect(()=>{
  if(!active)return;
  let current=true;
  void getTikTokRuntimeStatus().then(value=>{if(current)setStatus(value);}).catch(()=>{if(current)setStatus({state:"missing"});});
  return ()=>{current=false;};
 },[active]);
 return status;
}
export function TikTokComponentStatus({copy}:{copy:Catalog}){
 const status=useTikTokComponent();
 const label=status===null?copy.componentsLoading:status.state==="ready"?copy.componentReady:status.state==="missing"?copy.componentMissing:copy.componentNeedsRepair;
 return <ul className="local-component-list"><li><span>tiktok-core</span><span>{status?.version??"—"}</span><span className={status?.state==="ready"?"component-state ready":"component-state"}>{label}</span></li></ul>;
}
