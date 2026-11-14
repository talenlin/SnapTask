// Anchored, non-modal help. No application commands, synthesized clicks or field writes.
export function placeCoach(rect,size,viewport) {
  const edge=12,gap=12,w=Math.min(size.width,viewport.width-edge*2),h=Math.min(size.height,viewport.height-edge*2);
  const choices=[{side:'right',x:rect.right+gap,y:rect.top},{side:'left',x:rect.left-gap-w,y:rect.top},{side:'bottom',x:rect.left,y:rect.bottom+gap},{side:'top',x:rect.left,y:rect.top-gap-h}];
  const clamp=(n,min,max)=>Math.max(min,Math.min(n,Math.max(min,max)));
  for(const c of choices){c.x=clamp(c.x,edge,viewport.width-w-edge);c.y=clamp(c.y,edge,viewport.height-h-edge);c.overlap=Math.max(0,Math.min(c.x+w,rect.right)-Math.max(c.x,rect.left))*Math.max(0,Math.min(c.y+h,rect.bottom)-Math.max(c.y,rect.top));}
  choices.sort((a,b)=>a.overlap-b.overlap);return {...choices[0],width:w,height:h};
}
export function readCoachState(storage,key,tracks) {
  let raw;try{raw=JSON.parse(storage?.getItem(key)||'null');}catch{/* optional preferences */}
  const positions={};for(const [name,steps] of Object.entries(tracks))positions[name]=steps.some(s=>s.id===raw?.positions?.[name])?raw.positions[name]:steps[0].id;
  return {positions,dismissed:raw?.dismissed===true,completed:Array.isArray(raw?.completed)?raw.completed.filter(x=>Object.hasOwn(tracks,x)):[]};
}
export function createContextCoach({document:doc=document,window:win=window,storage,key,tracks,canStart=()=>true,onChange=()=>{},onError=()=>{}}) {
  const state=readCoachState(storage,key,tracks),node=(tag,cls,text)=>{const n=doc.createElement(tag);n.className=cls;if(text)n.textContent=text;return n;};
  const card=node('section','coach-bubble'),ring=node('div','coach-ring'),arrow=node('span','coach-arrow');
  card.id=`coach-${key.replace(/[^a-z0-9]/gi,'-')}`;card.setAttribute('role','region');card.setAttribute('aria-label','当前操作提示');card.setAttribute('popover','manual');ring.setAttribute('popover','manual');ring.setAttribute('aria-hidden','true');card.hidden=ring.hidden=true;
  const head=node('div','coach-head'),counter=node('p','coach-counter'),close=node('button','coach-close','退出引导'),title=node('h2','coach-title'),text=node('p','coach-text'),footer=node('div','coach-footer'),prev=node('button','','上一步'),next=node('button','coach-next','下一处');
  close.type=prev.type=next.type='button';title.tabIndex=-1;text.id=`${card.id}-text`;counter.setAttribute('aria-live','polite');head.append(counter,close);footer.append(prev,next);card.append(arrow,head,title,text,footer);doc.body.append(ring,card);
  let active=null,anchor=null,returnFocus=null,frame=0,menu=null,shown=false,paused=false;
  const persist=()=>{try{storage?.setItem(key,JSON.stringify(state));if(!storage)throw Error('unavailable');}catch{onError('引导进度未能保存在本机；不影响任务使用。');}onChange(state);};
  const visible=n=>!!n?.isConnected&&n.getClientRects().length>0&&win.getComputedStyle(n).visibility!=='hidden'&&!n.closest('[hidden]');
  function setAnchor(n){if(anchor===n)return;if(anchor){const ids=(anchor.getAttribute('aria-describedby')||'').split(/\s+/).filter(x=>x&&x!==text.id);if(ids.length)anchor.setAttribute('aria-describedby',ids.join(' '));else anchor.removeAttribute('aria-describedby');}anchor=n;if(anchor){const ids=new Set((anchor.getAttribute('aria-describedby')||'').split(/\s+/).filter(Boolean));ids.add(text.id);anchor.setAttribute('aria-describedby',[...ids].join(' '));}}
  function layer(n,open){if(open){n.hidden=false;if(n.showPopover&&!n.matches(':popover-open'))n.showPopover();}else if(!n.hidden){if(n.hidePopover&&n.matches(':popover-open'))n.hidePopover();n.hidden=true;}}
  function hide(){shown=false;layer(card,false);layer(ring,false);setAnchor(null);}
  function current(){return tracks[active]?.find(s=>s.id===state.positions[active]);}
  function resolve(step){return step?.resolve?step.resolve(doc):{target:doc.querySelector(step?.target||':not(*)')};}
  function refresh(){frame=0;if(!active&&!menu)return;if(paused){hide();return;}const step=current(),info=menu?{target:menu.target,title:'想了解哪一步？',text:'选择一项，在实际控件旁跟随操作。'}:resolve(step),target=info?.target,dialog=doc.querySelector('dialog[open]');
    if(!visible(target)||(dialog&&!dialog.contains(target))){hide();return;}
    const rect=target.getBoundingClientRect();if(rect.bottom<=0||rect.top>=win.innerHeight||rect.right<=0||rect.left>=win.innerWidth){hide();return;}
    // A modal dialog makes nodes outside it inert, even a visible top-layer popover.
    // Keep the hint inside the live dialog's focus tree, without making it modal itself.
    const host=target.closest('dialog[open]')||doc.body;
    if(card.parentElement!==host){layer(card,false);layer(ring,false);host.append(ring,card);}
    setAnchor(target);
    const index=active?tracks[active].indexOf(step):0;card.dataset.track=active||'menu';card.dataset.step=step?.id||'menu';counter.textContent=menu?'操作引导':`${index+1} / ${tracks[active].length} · ${step.group||'操作提示'}`;
    title.textContent=info.title||step?.title;text.textContent=info.text||step?.text;footer.hidden=!!menu;prev.disabled=index===0;next.textContent=info.nextLabel||step?.nextLabel||(index===tracks[active]?.length-1?'完成引导':'下一处');next.disabled=info.blocked===true;
    layer(ring,true);layer(card,true);const b=placeCoach(rect,{width:card.offsetWidth,height:card.offsetHeight},{width:win.innerWidth,height:win.innerHeight});card.style.left=`${b.x}px`;card.style.top=`${b.y}px`;card.dataset.side=b.side;
    const left=Math.max(3,rect.left-4),top=Math.max(3,rect.top-4);Object.assign(ring.style,{left:`${left}px`,top:`${top}px`,width:`${Math.max(0,Math.min(win.innerWidth-3,rect.right+4)-left)}px`,height:`${Math.max(0,Math.min(win.innerHeight-3,rect.bottom+4)-top)}px`});
    card.style.setProperty('--coach-arrow-x',`${Math.max(20,Math.min(card.offsetWidth-20,(rect.left+rect.right)/2-b.x))}px`);card.style.setProperty('--coach-arrow-y',`${Math.max(20,Math.min(card.offsetHeight-20,(rect.top+rect.bottom)/2-b.y))}px`);shown=true;
  }
  function schedule(){if(!frame)frame=win.requestAnimationFrame(refresh);}
  function go(id,focus=true){if(!active||!tracks[active].some(s=>s.id===id))return;state.positions[active]=id;persist();const target=resolve(current())?.target;if(visible(target))target.scrollIntoView({block:'nearest',inline:'nearest',behavior:'instant'});refresh();if(focus&&shown)title.focus({preventScroll:true});}
  function stop(complete=false){if(complete&&active&&!state.completed.includes(active))state.completed.push(active);active=null;menu=null;card.querySelector('.coach-menu')?.remove();hide();persist();if(visible(returnFocus))returnFocus.focus({preventScroll:true});}
  function start(name,reset=false){if(!Object.hasOwn(tracks,name)||!canStart())return;hide();menu=null;card.querySelector('.coach-menu')?.remove();active=name;returnFocus=doc.activeElement;if(reset)state.positions[name]=tracks[name][0].id;go(state.positions[name]);}
  function advance(){if(!active)return;const step=current(),info=resolve(step);if(info?.blocked)return;const steps=tracks[active],index=steps.indexOf(step),id=info?.next||step.next||steps[index+1]?.id;if(id)go(id);else stop(true);}
  next.onclick=advance;prev.onclick=()=>{const index=tracks[active]?.indexOf(current());if(index>0)go(tracks[active][index-1].id);};close.onclick=()=>stop();
  card.addEventListener('keydown',e=>e.stopPropagation());
  doc.addEventListener('keydown',e=>{if((active||menu)&&e.key==='Escape'){e.preventDefault();e.stopImmediatePropagation();stop();}},true);
  doc.addEventListener('click',e=>{if(!active||card.contains(e.target))return;const step=current(),match=step.clickTarget&&e.target.closest(step.clickTarget);if(match&&visible(match))queueMicrotask(()=>{if(active&&current()===step&&(!step.clickWhen||step.clickWhen(doc)))go(step.clickNext,false);});else schedule();});
  for(const name of ['scroll','resize'])win.addEventListener(name,schedule,true);doc.addEventListener('input',schedule);doc.addEventListener('change',schedule);win.addEventListener('blur',()=>{paused=true;hide();});win.addEventListener('focus',()=>{paused=false;schedule();});
  new win.MutationObserver(changes=>{if(changes.some(c=>!card.contains(c.target)&&!ring.contains(c.target)&&c.target!==card&&c.target!==ring))schedule();}).observe(doc.body,{subtree:true,childList:true,attributes:true,characterData:true});
  return {state,start,stop,notify(event){if(active&&current()?.event===event)go(current().eventNext,false);},menu(target,entries){if(!canStart())return;hide();active=null;returnFocus=doc.activeElement;menu={target};card.querySelector('.coach-menu')?.remove();const list=node('div','coach-menu');for(const [name,label] of entries){const b=node('button','',label);b.type='button';b.onclick=()=>start(name,true);list.append(b);}card.insertBefore(list,footer);refresh();title.focus({preventScroll:true});},dismiss(){state.dismissed=true;persist();},active:()=>!!active||!!menu};
}
