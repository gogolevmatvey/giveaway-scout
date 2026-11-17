import assert from 'node:assert/strict';

// Read real rendered bounds, not the virtualizer's private range or estimates.
export const historyViewport = browser => browser.execute(() => {
  const list=document.querySelector('.history-list');
  const bounds=list.getBoundingClientRect();
  const rows=[...list.querySelectorAll('.history-item')].map(item=>{
    const rect=item.getBoundingClientRect();
    return {id:item.dataset.notificationId,title:item.querySelector('strong').textContent,
      top:rect.top-bounds.top,bottom:rect.bottom-bounds.top,height:rect.height,
      position:Number(item.getAttribute('aria-posinset')),size:Number(item.getAttribute('aria-setsize'))};
  });
  return {top:list.scrollTop,height:list.clientHeight,scrollHeight:list.scrollHeight,
    busy:list.getAttribute('aria-busy')==='true',loadedCount:Number(list.dataset.historyLoadedCount),
    mounted:rows,visible:rows.filter(row=>row.bottom>0&&row.top<list.clientHeight),
    atBottom:list.scrollTop+list.clientHeight>=list.scrollHeight-2,
    viewport:{x:Math.round(bounds.left+bounds.width/2),y:Math.round(bounds.top+bounds.height/2)}};
});

// A delayed ResizeObserver can change estimated offsets after a scroll. Wait
// for actual idle geometry across frames, rather than guessing a millisecond
// delay or silently scrolling back to recover skipped records.
export async function settledHistoryViewport(browser,{timeout=5000}={}) {
  const settling=await browser.execute(async timeout=>{
    const started=performance.now(),samples=[];
    let previous='',stable=0;
    while(performance.now()-started<timeout) {
      await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
      const list=document.querySelector('.history-list');
      if(!list)continue;
      const bounds=list.getBoundingClientRect(),round=value=>Math.round(value*2)/2;
      const sample={elapsed:Math.round(performance.now()-started),top:round(list.scrollTop),height:list.clientHeight,width:list.clientWidth,
        scrollHeight:list.scrollHeight,busy:list.getAttribute('aria-busy')==='true',loadedCount:Number(list.dataset.historyLoadedCount),
        mounted:[...list.querySelectorAll('.history-item')].map(row=>{const rect=row.getBoundingClientRect();return {id:row.dataset.notificationId,position:Number(row.getAttribute('aria-posinset')),top:round(rect.top-bounds.top),height:round(rect.height)};})};
      const visible=sample.mounted.filter(row=>row.top+row.height>0&&row.top<sample.height);
      const first=visible[0],last=visible.at(-1),atBottom=sample.top+sample.height>=sample.scrollHeight-2;
      sample.visiblePositions=visible.map(row=>row.position);
      sample.covered=Boolean(first&&last&&first.top<=9&&(last.top+last.height>=sample.height-9||atBottom)&&
        visible.every((row,index)=>index===0||row.position===visible[index-1].position+1));
      samples.push(sample);
      const signature=JSON.stringify({...sample,elapsed:0});
      stable=!sample.busy&&sample.covered&&signature===previous?stable+1:0;
      previous=signature;
      if(stable>=3)return {settled:true,samples};
    }
    return {settled:false,samples};
  },timeout);
  if(!settling.settled) {
    const error=new Error('history geometry did not settle: '+JSON.stringify(settling.samples.slice(-6)));
    error.historySettling=settling;throw error;
  }
  return {...await historyViewport(browser),settling};
}

export async function scrollHistory(browser,delta,{wheel=true,settle=false}={}) {
  if(wheel) {
    const {viewport}=await historyViewport(browser);
    await browser.performActions([{type:'wheel',id:'history-wheel',actions:[
      {type:'scroll',origin:'viewport',...viewport,deltaX:0,deltaY:Math.round(delta),duration:80},
    ]}]);
    await browser.releaseActions();
  } else {
    // Native WebDriver has no wheel action; DOM scrollBy invokes WebView2's
    // ordinary scroll/measurement path without dispatching synthetic events.
    await browser.execute(amount=>document.querySelector('.history-list').scrollBy(0,amount),delta);
  }
  if(settle)return settledHistoryViewport(browser);
  await browser.pause(60);
}

export async function walkHistory(browser,lastId,{wheel=true,maxSteps=220,onStep}={}) {
  const seen=new Set(),heights=new Set();
  const trace=[];
  let largestDom=0,last=await settledHistoryViewport(browser);
  const summary=view=>({top:view.top,height:view.height,scrollHeight:view.scrollHeight,busy:view.busy,loadedCount:view.loadedCount,
    visible:view.visible.map(({id,position,top,bottom,height})=>({id,position,top,bottom,height})),
    mounted:view.mounted.map(({id,position,top,height})=>({id,position,top,height}))});
  for(let step=0;step<maxSteps;step++) {
    const ids=last.mounted.map(row=>row.id);
    assert.equal(new Set(ids).size,ids.length,'no duplicate mounted records');
    assert.ok(ids.length<=30,'mounted history is bounded: '+JSON.stringify({count:ids.length,top:last.top}));
    largestDom=Math.max(largestDom,ids.length);
    for(const row of last.visible) {seen.add(row.id);heights.add(Math.round(row.height));}
    if(last.atBottom&&last.visible.some(row=>row.id===lastId)) return {seen:[...seen],heights:[...heights],largestDom,last,trace};
    const delta=Math.max(100,last.height*0.7),before=last;
    try {last=await scrollHistory(browser,delta,{wheel,settle:true});}
    catch(error) {
      const failed={step,requestedDelta:delta,before:summary(before),error:error.message,settling:error.historySettling};
      trace.push(failed);if(onStep)await onStep(failed);throw error;
    }
    const entry={step,requestedDelta:delta,before:summary(before),after:summary(last),settling:last.settling};
    trace.push(entry);if(onStep)await onStep(entry);
    const previousLast=before.visible.at(-1),nextFirst=last.visible[0];
    assert.ok(previousLast&&nextFirst&&nextFirst.position<=previousLast.position+1,
      'forward scrolling skipped a logical record range: '+JSON.stringify(entry));
  }
  assert.fail('Last history record was not reached: '+JSON.stringify(last));
}

export const historyAnchor = viewport => {
  const row=viewport.visible.find(row=>row.top>=0&&row.bottom<=viewport.height)??viewport.visible[0];
  assert.ok(row,'a visible history anchor exists');
  return {id:row.id,top:row.top};
};
