const express=require('express');
const http=require('http');
const {WebSocketServer}=require('ws');
const app=express();const s=http.createServer(app);const wss=new WebSocketServer({server:s});
app.use(express.static('public'));app.use(express.json());
const users={},rooms={},MAP_W=3000,MAP_H=3000;
function uid(){return Math.random().toString(36).slice(2,10);}
function dist(a,b){return Math.hypot(a.x-b.x,a.y-b.y);}
function send(ws,msg){if(ws.readyState===1)ws.send(JSON.stringify(msg));}
app.post('/api/register',(req,res)=>{const{name,password}=req.body;if(!name||!password||users[name])return res.json({ok:false});users[name]={name,password,gold:500,wins:0,kills:0};res.json({ok:true});});
app.post('/api/login',(req,res)=>{const u=users[req.body.name];if(!u||u.password!==req.body.password)return res.json({ok:false});const t=uid();u.token=t;res.json({ok:true,token:t,gold:u.gold});});
function createRoom(){const room={id:uid(),players:[],state:'waiting',circle:{x:MAP_W/2,y:MAP_H/2,r:1800},items:[],projectiles:[]};for(let i=0;i<60;i++)room.items.push({x:100+Math.random()*(MAP_W-200),y:100+Math.random()*(MAP_H-200),type:Math.random()<.3?'health':'ammo'});rooms[room.id]=room;return room;}
function findRoom(){for(const id in rooms){const r=rooms[id];if(r.state==='waiting')return r;}return createRoom();}
function broadcast(room,msg){room.players.forEach(p=>{if(p.ws.readyState===1)send(p.ws,msg);});}
function tickRoom(room){
  if(room.state!=='playing')return;
  room.circle.r=Math.max(100,room.circle.r-0.3);
  room.players.forEach(p=>{
    if(!p.alive)return;
    if(p.speedX||p.speedY){p.x+=p.speedX*4;p.y+=p.speedY*4;p.x=Math.max(20,Math.min(MAP_W-20,p.x));p.y=Math.max(20,Math.min(MAP_H-20,p.y));}
    if(dist(p,room.circle)>room.circle.r){p.hp-=1.5;if(p.hp<=0)p.alive=false;}
    if(p.shooting&&Date.now()-(p.lastShot||0)>300){p.lastShot=Date.now();room.projectiles.push({x:p.x,y:p.y,vx:Math.cos(p.shootAngle),vy:Math.sin(p.shootAngle),shooter:p.id,dmg:25,life:50,speed:10});}
    for(let i=room.items.length-1;i>=0;i--){if(dist(p,room.items[i])<28){if(room.items[i].type==='health')p.hp=Math.min(100,p.hp+25);room.items.splice(i,1);break;}}
  });
  for(let i=room.projectiles.length-1;i>=0;i--){const pr=room.projectiles[i];pr.x+=pr.vx*pr.speed;pr.y+=pr.vy*pr.speed;pr.life--;if(pr.life<=0){room.projectiles.splice(i,1);continue;}for(const p of room.players){if(!p.alive||p.id===pr.shooter)continue;if(dist(pr,p)<22){p.hp-=pr.dmg;room.projectiles.splice(i,1);break;}}}
  const alive=room.players.filter(p=>p.alive);
  if(alive.length<=1&&room.players.length>1){room.state='ended';if(alive[0])broadcast(room,{type:'win',winner:alive[0].name});setTimeout(()=>delete rooms[room.id],10000);}
}
wss.on('connection',(ws)=>{
  ws.playerId=uid();
  ws.on('message',(raw)=>{let msg;try{msg=JSON.parse(raw);}catch(e){return;}
    if(msg.type==='auth'){const u=users[msg.name];if(!u||u.token!==msg.token){send(ws,{type:'error'});return;}ws.userName=msg.name;}
    if(msg.type==='join'){if(!ws.userName)return;const room=findRoom();room.players.push({id:ws.playerId,name:ws.userName,x:0,y:0,hp:100,alive:true,kills:0,speedX:0,speedY:0,lastShot:0,shooting:false,shootAngle:0,ws});ws.roomId=room.id;send(ws,{type:'joined'});if(room.players.length>=2&&room.state==='waiting'){setTimeout(()=>{room.state='playing';room.players.forEach((p,i)=>{p.x=500+(i%4)*600;p.y=500+Math.floor(i/4)*600;p.hp=100;});broadcast(room,{type:'start'});},2000);}}
    if(msg.type==='move'){if(!ws.roomId)return;const room=rooms[ws.roomId];if(!room)return;const p=room.players.find(x=>x.id===ws.playerId);if(p&&p.alive){p.speedX=msg.x||0;p.speedY=msg.y||0;}}
    if(msg.type==='shoot'){if(!ws.roomId)return;const room=rooms[ws.roomId];if(!room)return;const p=room.players.find(x=>x.id===ws.playerId);if(p&&p.alive){p.shooting=msg.active;if(msg.angle!==undefined)p.shootAngle=msg.angle;}}
  });
});
setInterval(()=>{for(const id in rooms)tickRoom(rooms[id]);},16);
s.listen(3001,()=>console.log('Server2 lite on :3001'));
