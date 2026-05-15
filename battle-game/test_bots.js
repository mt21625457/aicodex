const WebSocket=require('ws');
const http=require('http');
const HOST='localhost',PORT=3000;
const BOTS=10;
const NAMES=['Alpha','Bravo','Charlie','Delta','Echo','Foxtrot','Golf','Hotel','India','Juliet'];

function api(path,body){
  return new Promise((resolve,reject)=>{
    const d=JSON.stringify(body);
    const req=http.request({hostname:HOST,port:PORT,path,method:'POST',headers:{'Content-Type':'application/json'}},(res)=>{
      let b='';res.on('data',c=>b+=c);res.on('end',()=>resolve(JSON.parse(b)));
    });
    req.on('error',reject);req.write(d);req.end();
  });
}

async function ensureUser(name){
  let r=await api('/api/login',{name,password:'bot123'});
  if(!r.ok)r=await api('/api/register',{name,password:'bot123'});
  if(!r.token){r=await api('/api/register',{name,password:'bot123'});r=await api('/api/login',{name,password:'bot123'});}
  return r;
}

function createBot(name,token,char,mode,delay){
  return new Promise((resolve)=>{
    setTimeout(()=>{
      const ws=new WebSocket(`ws://${HOST}:${PORT}`);
      ws.on('open',()=>{ws.send(JSON.stringify({type:'auth',name,token}));});
      let joined=false,started=false,lastShot=0,shootCD=200+Math.random()*400;
      let myState=null,players=[],circle={x:1500,y:1500,r:1800};
      ws.on('message',(raw)=>{
        const m=JSON.parse(raw);
        if(m.type==='joined')joined=true;
        if(m.type==='start')started=true;
        if(m.type==='state'&&started&&m.me?.alive){
          myState=m.me;players=m.players;circle=m.circle;
          const now=Date.now();
          var targetX=circle.x,targetY=circle.y;
          var nearest=null,nearestDist=9999,nearestWeak=null,weakDist=9999;
          m.players.forEach(p=>{
            if(!p.alive||p.id===m.me.id)return;
            var d=Math.hypot(p.x-m.me.x,p.y-m.me.y);
            if(d<nearestDist){nearestDist=d;nearest=p;}
            if(p.hp<40&&d<weakDist){weakDist=d;nearestWeak=p;}
          });
          if(nearestWeak&&weakDist<250)targetX=nearestWeak.x;targetY=nearestWeak.y;
          else if(nearest&&nearestDist<350)targetX=nearest.x;targetY=nearest.y;
          else{var dCircle=Math.hypot(m.me.x-circle.x,m.me.y-circle.y);if(dCircle>circle.r-150){targetX=circle.x;targetY=circle.y;}}
          var dx=targetX-m.me.x,dy=targetY-m.me.y;var mag=Math.hypot(dx,dy)||1;
          if(Math.random()<.15){dx=(Math.random()-.5)*2;dy=(Math.random()-.5)*2;mag=Math.hypot(dx,dy)||1;}
          ws.send(JSON.stringify({type:'move',x:dx/mag,y:dy/mag}));
          if(nearest&&nearestDist<380&&now-lastShot>shootCD){
            lastShot=now;shootCD=180+Math.random()*350;
            var a=Math.atan2(nearest.y-m.me.y,nearest.x-m.me.x)+(Math.random()-.5)*.25;
            ws.send(JSON.stringify({type:'shoot',active:true,angle:a}));
            setTimeout(()=>ws.send(JSON.stringify({type:'shoot',active:false})),80);
          }
          if(nearestDist<180&&Math.random()<.06)ws.send(JSON.stringify({type:'grenade',angle:Math.atan2(nearest.y-m.me.y,nearest.x-m.me.x)}));
          if(Math.random()<.008)ws.send(JSON.stringify({type:'emote',emoji:['💪','🎉','👀','🔥'][Math.floor(Math.random()*4)]}));
          if(m.me.weapon==='pistol'&&Math.random()<.02)ws.send(JSON.stringify({type:'switchweapon',weapon:'rifle'}));
        }
        if(m.type==='killstreak')console.log(`🔥 ${m.player} ${m.streak}连杀!`);
        if(m.type==='win'){console.log(`🏆 ${m.winner} wins (${m.kills}k)`);ws.close();resolve();}
      });
      ws.on('close',()=>resolve());
      ws.on('error',()=>resolve());
    },delay);
  });
}

async function main(){
  console.log('🎮 Battle Royale — 10 Smart Bots v2\n');
  const chars=['soldier','ninja','tank','soldier','ninja','tank','soldier','ninja','tank','soldier'];
  const bots=[];
  for(let i=0;i<BOTS;i++){
    const name=NAMES[i];const r=await ensureUser(name);
    console.log(`✅ ${name} (${r.gold}g)`);
    bots.push(createBot(name,r.token,chars[i],'solo',i*150));
  }
  console.log(`\n🤖 ${BOTS} smart bots joining...\n`);
  await Promise.all(bots);
  console.log('\n✅ Match complete!');
  process.exit(0);
}
main().catch(e=>{console.error(e);process.exit(1);});
