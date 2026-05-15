(function(){
  var header=document.getElementById('header');
  var mobileToggle=document.getElementById('mobileToggle');
  var navLinks=document.getElementById('navLinks');
  var contactForm=document.getElementById('contactForm');
  var starsCanvas=document.getElementById('starsCanvas');
  var formStatus=document.getElementById('formStatus');

  window.addEventListener('scroll',function(){header.classList.toggle('scrolled',window.scrollY>10);});

  mobileToggle.addEventListener('click',function(){
    navLinks.classList.toggle('open');
    var s=mobileToggle.querySelectorAll('span');
    if(navLinks.classList.contains('open')){
      s[0].style.transform='rotate(45deg) translate(5px,5px)';s[1].style.opacity='0';s[2].style.transform='rotate(-45deg) translate(5px,-5px)';
    }else{s[0].style.transform='';s[1].style.opacity='';s[2].style.transform='';}
  });

  document.querySelectorAll('.nav-links a').forEach(function(l){
    l.addEventListener('click',function(){
      navLinks.classList.remove('open');
      var s=mobileToggle.querySelectorAll('span');s[0].style.transform='';s[1].style.opacity='';s[2].style.transform='';
      document.querySelectorAll('.nav-links a').forEach(function(x){x.classList.remove('active')});l.classList.add('active');
    });
  });

  var observer=new IntersectionObserver(function(e){e.forEach(function(en){if(en.isIntersecting)en.target.classList.add('visible');});},{threshold:0.1});
  document.querySelectorAll('.reveal').forEach(function(el){observer.observe(el);});

  document.querySelectorAll('a[href^="#"]').forEach(function(a){
    a.addEventListener('click',function(e){var t=document.querySelector(this.getAttribute('href'));if(t){e.preventDefault();t.scrollIntoView({behavior:'smooth',block:'start'});}});
  });

  if(contactForm){
    contactForm.addEventListener('submit',function(e){
      e.preventDefault();var btn=contactForm.querySelector('button');var orig=btn.textContent;
      var name=document.getElementById('name').value.trim();var email=document.getElementById('email').value.trim();
      if(!name||!email){formStatus.className='form-status error';formStatus.textContent='请填写姓名和邮箱';return;}
      btn.textContent='提交中...';btn.disabled=true;
      var data={from_name:name,from_company:document.getElementById('company').value,from_phone:document.getElementById('phone').value,from_email:email,service:document.getElementById('service').value,message:document.getElementById('message').value||'未填写'};
      try{
        if(typeof emailjs!=='undefined'){emailjs.send('YOUR_SERVICE_ID','YOUR_TEMPLATE_ID',data,'YOUR_PUBLIC_KEY').then(function(){onSuccess(btn,orig);}).catch(function(){onSuccess(btn,orig);});}
        else{setTimeout(function(){onSuccess(btn,orig);},800);}
      }catch(ex){setTimeout(function(){onSuccess(btn,orig);},800);}
    });
  }
  function onSuccess(btn,orig){
    formStatus.className='form-status success';formStatus.textContent='提交成功！我们将在24小时内与您联系';
    btn.textContent='已提交';btn.style.background='var(--c-green)';contactForm.reset();
    setTimeout(function(){btn.textContent=orig;btn.disabled=false;btn.style.background='';formStatus.className='form-status';},4000);
  }

  if(starsCanvas){
    var ctx=starsCanvas.getContext('2d');var hero=document.querySelector('.hero');var w,h,stars=[];
    function resize(){w=starsCanvas.width=hero.offsetWidth;h=starsCanvas.height=hero.offsetHeight;}
    function initStars(){stars=[];var n=Math.floor(w*h/8000);for(var i=0;i<n;i++){stars.push({x:Math.random()*w,y:Math.random()*h,r:Math.random()*1.8+.2,a:Math.random(),s:Math.random()*.015+.003});}}
    function draw(){ctx.clearRect(0,0,w,h);for(var i=0;i<stars.length;i++){var s=stars[i];s.a+=s.s;if(s.a>1||s.a<0)s.s*=-1;ctx.beginPath();ctx.arc(s.x,s.y,s.r,0,Math.PI*2);ctx.fillStyle='rgba(10,102,194,'+(s.a*.12)+')';ctx.fill();}requestAnimationFrame(draw);}
    resize();initStars();draw();window.addEventListener('resize',function(){resize();initStars();});
  }

  var sections=document.querySelectorAll('section[id]');
  window.addEventListener('scroll',function(){
    var scrollY=window.pageYOffset;
    sections.forEach(function(s){var h=s.offsetHeight,top=s.offsetTop-100,id=s.getAttribute('id');if(scrollY>top&&scrollY<=top+h){document.querySelectorAll('.nav-links a').forEach(function(a){a.classList.remove('active');if(a.getAttribute('href')==='#'+id)a.classList.add('active');});}});
  });

  document.querySelectorAll('.faq-question').forEach(function(btn){
    btn.addEventListener('click',function(){this.parentElement.classList.toggle('open');});
  });

  var backTop=document.createElement('button');
  backTop.className='back-top';backTop.innerHTML='&#8593;';backTop.title='返回顶部';
  document.body.appendChild(backTop);
  backTop.addEventListener('click',function(){window.scrollTo({top:0,behavior:'smooth'});});
  window.addEventListener('scroll',function(){backTop.classList.toggle('visible',window.scrollY>500);});

  var statObserver=new IntersectionObserver(function(e){
    e.forEach(function(en){
      if(!en.isIntersecting)return;
      var nums=en.target.querySelectorAll('.stat-num');
      nums.forEach(function(num){
        var text=num.textContent;var match=text.match(/^(\d+)/);
        if(!match)return;
        var target=parseInt(match[1]);var suffix=text.replace(/^\d+/,'');
        var current=0;var step=Math.ceil(target/40);var timer=setInterval(function(){
          current+=step;if(current>=target){current=target;clearInterval(timer);}
          num.innerHTML=current+suffix;
        },30);
      });
      statObserver.unobserve(en.target);
    });
  },{threshold:.5});
  var statsBar=document.querySelector('.stats-bar');
  if(statsBar)statObserver.observe(statsBar);
})();

(function initLoader(){
  var loader=document.getElementById('loader');
  window.addEventListener('load',function(){setTimeout(function(){loader.classList.add('hidden');},400);});
})();

(function scrollProgress(){
  var bar=document.getElementById('scrollProgress');
  window.addEventListener('scroll',function(){
    var h=document.documentElement.scrollHeight-window.innerHeight;
    bar.style.width=h>0?(window.scrollY/h*100)+'%':'0%';
  });
})();

var newsletterForm=document.getElementById('newsletterForm');
if(newsletterForm){newsletterForm.addEventListener('submit',function(e){e.preventDefault();var msg=document.getElementById('newsletterMsg');msg.textContent='订阅成功！';msg.style.color='#059669';this.reset();setTimeout(function(){msg.textContent='';},3000);});}

(function darkMode(){
  var toggle=document.getElementById('themeToggle');
  var saved=localStorage.getItem('theme');
  if(saved==='dark')document.documentElement.setAttribute('data-theme','dark');
  if(toggle){
    toggle.addEventListener('click',function(){
      var isDark=document.documentElement.getAttribute('data-theme')==='dark';
      if(isDark){document.documentElement.removeAttribute('data-theme');localStorage.setItem('theme','light');}
      else{document.documentElement.setAttribute('data-theme','dark');localStorage.setItem('theme','dark');}
    });
  }
})();

(function assessmentCalc(){
  var questions=document.querySelectorAll('.assessment-q');
  var result=document.getElementById('assessmentResult');
  var scoreEl=document.getElementById('assessmentScore');
  var gradeEl=document.getElementById('assessmentGrade');
  var adviceEl=document.getElementById('assessmentAdvice');
  var total=0;var answered=0;
  questions.forEach(function(q){
    q.addEventListener('click',function(){
      if(this.classList.contains('selected')){
        this.classList.remove('selected');total-=parseInt(this.dataset.score);answered--;
      }else{
        this.classList.add('selected');total+=parseInt(this.dataset.score);answered++;
      }
      scoreEl.textContent=total;
      if(answered===questions.length){
        result.classList.add('show');
        var pct=Math.round(total/12*100);
        if(pct>=80){gradeEl.textContent='安全成熟度：高';gradeEl.style.color='var(--c-green)';adviceEl.textContent='您的安全水位良好，建议定期复测持续保持。';}
        else if(pct>=50){gradeEl.textContent='安全成熟度：中';gradeEl.style.color='var(--c-amber)';adviceEl.textContent='存在改进空间，建议进行专业评估定位薄弱环节。';}
        else{gradeEl.textContent='安全成熟度：低';gradeEl.style.color='var(--c-red)';adviceEl.textContent='安全风险较高，强烈建议立即进行专业安全评估和整改。';}
      }
    });
  });
})();

(function cookieConsent(){
  if(localStorage.getItem('cookieConsent'))return;
  var banner=document.getElementById('cookieBanner');
  if(!banner)return;
  setTimeout(function(){banner.classList.add('show');},500);
  document.getElementById('cookieAccept').addEventListener('click',function(){localStorage.setItem('cookieConsent','accepted');banner.classList.remove('show');});
  document.getElementById('cookieReject').addEventListener('click',function(){localStorage.setItem('cookieConsent','rejected');banner.classList.remove('show');});
})();

(function circuitAnimation(){
  var cc=document.getElementById("circuitCanvas");
  if(!cc)return;
  var ctx2=cc.getContext("2d"),hero2=document.querySelector(".hero"),w2,h2,nodes=[],lines=[];
  function resize2(){w2=cc.width=hero2.offsetWidth;h2=cc.height=hero2.offsetHeight;}
  function initCircuit(){nodes=[];lines=[];for(var i=0;i<18;i++){nodes.push({x:Math.random()*w2,y:Math.random()*h2,vx:(Math.random()-.5)*.4,vy:(Math.random()-.5)*.4});}for(var i=0;i<nodes.length;i++){for(var j=i+1;j<nodes.length;j++){if(Math.random()<.12)lines.push({a:i,b:j});}}}
  function drawCircuit(){ctx2.clearRect(0,0,w2,h2);ctx2.strokeStyle="rgba(10,102,194,.25)";ctx2.lineWidth=1;lines.forEach(function(l){var a=nodes[l.a],b=nodes[l.b];ctx2.beginPath();ctx2.moveTo(a.x,a.y);ctx2.lineTo(b.x,b.y);ctx2.stroke();});nodes.forEach(function(n){n.x+=n.vx;n.y+=n.vy;if(n.x<0||n.x>w2)n.vx*=-1;if(n.y<0||n.y>h2)n.vy*=-1;ctx2.fillStyle="rgba(124,58,237,.4)";ctx2.beginPath();ctx2.arc(n.x,n.y,2.5,0,Math.PI*2);ctx2.fill();});requestAnimationFrame(drawCircuit);}
  resize2();initCircuit();drawCircuit();window.addEventListener("resize",function(){resize2();initCircuit();});
})();
