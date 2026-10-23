const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { test } = require('node:test');
const vm = require('node:vm');
const source = readFileSync(require.resolve('../battle-game/server.js'), 'utf8');
const start = source.indexOf('  const alive=room.players.filter(p=>p.alive);if(alive.length<=1');
const end = source.indexOf('\n}\n\nfunction killPlayer', start);
assert(start >= 0 && end > start);
const optimized = source.slice(start, end);
// 保留独立的旧版字段生成作为差分基准，不依赖运行时 Git 或网络服务。
const reference = "  const alive=room.players.filter(p=>p.alive);if(alive.length<=1&&room.players.length>1)endRoom(room,alive[0]);\n  room.players.forEach(p=>{const alivePlayers=room.players.filter(x=>x.alive);var specIdx=-1,specTarget=null;if(!p.alive){const others=alivePlayers.filter(x=>x.id!==p.id);if(others.length>0){const idx=Math.min(p.spectateIdx||0,others.length-1);specTarget=others[idx];specIdx=idx;}}\n  send(p.ws,{type:'state',me:{x:p.x,y:p.y,hp:p.hp,armor:p.armor,ammo:p.ammo,weapon:p.weapon,kills:p.kills,grenades:p.grenades,alive:p.alive,spectateCount:alivePlayers.length-1,placement:p.placement},players:room.players.map(x=>({id:x.id,x:x.x,y:x.y,hp:x.hp,armor:x.armor,alive:x.alive,char:x.char,name:x.name,weapon:x.weapon,team:x.team,crouching:x.crouching,sprinting:x.sprinting})),circle:room.circle,items:room.items.filter(it=>dist(p,it)<500),projectiles:room.projectiles.filter(pr=>dist(p,pr)<550),grenades:room.grenades.filter(g=>dist(p,g)<500),airdrops:room.airdrops.filter(ad=>dist(p,ad)<800),particles:room.particles.filter(pt=>dist(p,pt)<600),kills:room.kills.slice(-5),emotes:room.emotes.slice(-10),pings:room.pings?room.pings.slice(-5):[],chats:room.chats.slice(-10),teammates:room.players.filter(x=>x.team===p.team&&x.id!==p.id).map(x=>({id:x.id,x:x.x,y:x.y,hp:x.hp,name:x.name})),aliveCount:alive.length,buildings:room.buildings,terrain:TERRAIN,specTarget:specTarget?{x:specTarget.x,y:specTarget.y,name:specTarget.name}:null,specIdx,gameTime:Date.now()-room.startTime});});";
function snapshot(code, room) {
  const messages = [];
  let clock = 100;
  let playerMapCalls = 0;
  const clonedRoom = structuredClone(room);
  clonedRoom.players.map = function(callback) {
    playerMapCalls++;
    return Array.prototype.map.call(this, callback);
  };
  vm.runInNewContext(code, {
    room: clonedRoom,
    send: (ws, msg) => messages.push(JSON.parse(JSON.stringify({ws, msg}))),
    endRoom: (r) => { r.state = 'ended'; r.players.forEach(p => { if(!p.placement) p.placement=p.alive?1:2; }); },
    dist: (a,b) => Math.hypot(a.x-b.x,a.y-b.y), TERRAIN: [[1,2],[3,0]],
    Date: {now: () => clock++},
  });
  return {messages, playerMapCalls};
}
for (const count of [0, 1, 2, 10, 40]) {
  for (const aliveCount of [0, 1, count]) {
    test(`state payload differential: ${count} players, ${aliveCount} alive`, () => {
      const entries = Array.from({length: 15}, (_, i) => ({x:i*100,y:i*70,id:i,text:`message ${i}`}));
      const room = {
        players: Array.from({length:count}, (_,i) => ({id:i,ws:i,x:i*100,y:i*80,hp:100-i,armor:i,ammo:20,weapon:'rifle',kills:i,grenades:i,alive:i<aliveCount,team:i%3,name:`p${i}`,char:'soldier',spectateIdx:i%4,crouching:i%2===0,sprinting:i%3===0})),
        circle:{x:500,y:500,r:1000},items:entries,projectiles:entries,grenades:entries,airdrops:entries,particles:entries,kills:entries,emotes:entries,pings:entries,chats:entries,buildings:[],startTime:10,
      };
      const actual = snapshot(optimized,room);
      const expected = snapshot(reference,room);
      assert.deepEqual(actual.messages, expected.messages);
      // 验收计算量：公共玩家投影由每位接收者各一次降为每 tick 一次。
      assert.equal(actual.playerMapCalls, 1);
      assert.equal(expected.playerMapCalls, count);
    });
  }
}
