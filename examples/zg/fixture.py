"""Synthetic ZG workflow fixture. Contains no captured customer information."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
from urllib.parse import urlparse

CONFIG={
    'origin':'东京（成田）','destination':'首尔（仁川）','departure_date':'2027-03-20',
    'departure_time':'09:00','flight_number':'ZG041','currency':'JPY','amount':'12000',
    'passenger':{'first_name':'TEST','last_name':'EXAMPLE','birthday':'1990-01-01',
      'gender':'M','nationality_label':'中国','email':'sdk@example.invalid','phone':'00000000000',
      'phone_code':'+86 (中国)','passport':'TEST00000','passport_expiry':'2030-01-01'}
}

HTML='''<!doctype html><html><head><meta charset="utf-8"></head><body><main></main>
<script>
const root=document.querySelector('main');let state=0;let selections={};
const button=(s,extra='')=>'<button '+extra+'>'+s+'</button>';
const link=s=>'<a href="#">'+s+'</a>';
const input=s=>'<label>'+s+'<input></label>';
function show(html){root.innerHTML=html;for(const a of root.querySelectorAll('a'))a.addEventListener('click',e=>e.preventDefault());}
const next=()=>{state++;render()};
function render(){
 switch(state){
 case 0:show(button('同意')+link('登录'));break;
 case 1:show(input('电子邮箱※')+input('密码※')+button('登录'));break;
 case 2:show(button('点击并更改货币。'));break;
 case 3:show(link('JPY'));break;
 case 4:show(button('单程包括中转','role="tab"'));break;
 case 5:show(button('出发地（国家，地区）'));break;
 case 6:show(button('东京（成田）'));break;
 case 7:show(button('目的地（国家，地区）'));break;
 case 8:show(button('首尔（仁川）'));break;
 case 9:show(button('搜索机票'));break;
 case 10:case 11:show(button('下一步'));break;
 case 12:show(button('20','data-fulldate="2027-03-20"'));break;
 case 13:
 show('<img alt="正在读取中">');
 fetch('/search').then(r=>r.json()).then(d=>{
 if(!d.flight){show('<p>没有航班</p>');return;}
 show('<div class="cabin-contents"><span>'+d.flight+'</span><span class="start">09:00</span>'+button('座位类型Standard')+'</div>');
 root.querySelector('button').onclick=next;});return;
 case 14:show(button('下一步'));break;
 case 15:show(link('不要买套餐'));break;
 case 16:
 show('<h1>输入乘客</h1><input id="lastName"><input id="firstName">'+button('男')+
 '<div role="group" aria-label="出生年月日">'+button('日')+button('月')+button('年')+'</div>'+
 button('国籍/地区※')+input('电子邮箱※(输入半角英文字母与数字)')+input('电子邮箱（确认）※')+
 button('国家/地区代码')+input('电话号码※(输入半角数字)')+input('护照号码※(输入半角英文字母与数字)')+
 '<div role="group" aria-label="有效期限">'+button('日')+button('月')+button('年')+'</div>'+link('下一步'));
 for(const b of root.querySelectorAll('button'))b.onclick=()=>{
  let values={'日':['1'],'月':['Jan'],'年':['1990','2030'],'国籍/地区※':['中国'],'国家/地区代码':['+86 (中国)']};
  if(!values[b.textContent])return;
  let pop=document.createElement('div');pop.innerHTML=values[b.textContent].map(v=>button(v,'role="option"')).join('');root.append(pop);
  for(const o of pop.querySelectorAll('button'))o.onclick=()=>pop.remove();
 };
 root.querySelector('a').onclick=()=>{selections.passenger=document.querySelector('#lastName').value+' '+document.querySelector('#firstName').value;next();};return;
 case 17:show(button('下一步'));break;
 case 18:show(button('我已确认购票、搭乘相关的注意事项。')+link('下一步'));root.querySelector('button').onclick=()=>{};root.querySelector('a').onclick=next;return;
 case 19:show('<label>直接输入<input type="radio"></label>');root.querySelector('input').onclick=()=>{
 let o=document.createElement('button');o.setAttribute('role','option');o.textContent=selections.passenger;root.append(o);o.onclick=()=>{show(link('下一步'));root.querySelector('a').onclick=next;};};return;
 case 20:
 fetch('/review').then(r=>r.json()).then(d=>{
 show('<div data-testid="review-flight">ZG041</div><div data-testid="review-passenger">'+selections.passenger+'</div><div data-testid="review-total">JPY '+d.amount+'</div>'+button('其他信用卡')+button('模拟支付'));
 root.querySelectorAll('button')[1].onclick=async()=>{await fetch('/v2/mypage/flights/member',{method:'POST',body:JSON.stringify(selections)});};});return;
 }
 for(const el of root.querySelectorAll('button,a')){
  if(el.textContent==='同意'){el.onclick=()=>el.remove();continue;}
  el.onclick=next;
 }
}
render();
</script></body></html>'''

class Fixture:
    def __init__(self,scenario='success'):
        self.scenario=scenario;self.payments=[]
    def __enter__(self):
        fixture=self
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_GET(self):
                path=urlparse(self.path).path
                if path=='/search':
                    body={'flight':None if fixture.scenario=='no_flight' else 'ZG999' if fixture.scenario=='wrong_flight' else 'ZG041'}
                elif path=='/review':body={'amount':13000 if fixture.scenario=='price_change' else 12000}
                else:
                    self.reply(200,HTML.encode(),'text/html; charset=utf-8');return
                self.reply(200,json.dumps(body).encode())
            def do_POST(self):
                data=json.loads(self.rfile.read(int(self.headers.get('Content-Length',0))))
                fixture.payments.append(data)
                body={'pnr':{'confirmationNumber':'TESTPNR','flightNumber':'ZG041','passenger':data['passenger'],'amount':12000,'currency':'JPY'}}
                if fixture.scenario=='payment_failure':self.reply(402,b'{"error":"declined"}');return
                self.reply(200,json.dumps(body).encode())
            def reply(self,status,body,content='application/json'):
                self.send_response(status);self.send_header('Content-Type',content);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
        self.server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        self.thread=Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.origin=f'http://127.0.0.1:{self.server.server_port}'
        return self
    def __exit__(self,*_):
        self.server.shutdown();self.server.server_close();self.thread.join()
