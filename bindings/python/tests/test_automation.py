"""Real isolated-runtime tests. Run with OBSCURA_RUNTIME_BIN set explicitly."""
import asyncio
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import tempfile
import threading
import unittest
from obscura_runtime import Browser, BrowserError, TimeoutError, expect, file_hash

PERSONA = {"schema_version":"1", "persona_id":"sdk_test", "revision":"1",
           "profile":os.environ.get("OBSCURA_TEST_PROFILE", "windows_chrome145"), "viewport":{"width":800,"height":600}}
HTML = '''<!doctype html><html><body>
<label for="name">Name</label><input id="name"><button id="send" disabled>Send</button><button style="display:none">Send</button>
<button id="duplicate">Duplicate</button><button>Duplicate</button>
<select aria-label="Currency"><option value="JPY">Yen</option><option value="USD">Dollar</option></select>
<label><input type="checkbox" id="terms">Terms</label>
<div id="result">Pending</div><div id="events"></div>
<div style="height:900px"></div><button id="bottom">Bottom</button>
<script>
window.inputLog=[];
for(const type of ['pointerdown','mousedown','pointerup','mouseup','click'])
 document.querySelector('#send').addEventListener(type,e=>{window.inputLog.push(type);document.querySelector('#events').textContent=window.inputLog.join(',')});
setTimeout(()=>{let old=document.querySelector('#send');let b=old.cloneNode(true);b.disabled=false;old.replaceWith(b);
for(const type of ['pointerdown','mousedown','pointerup','mouseup','click']) b.addEventListener(type,e=>{window.inputLog.push(type);document.querySelector('#events').textContent=window.inputLog.join(',')});
b.addEventListener('click',async()=>{const r=await fetch('/api',{method:'POST',body:document.querySelector('#name').value});let d=await r.json();document.querySelector('#result').textContent=d.result;});},150);
document.querySelector('#bottom').onclick=()=>document.querySelector('#result').textContent='Bottom clicked';
</script></body></html>'''

class Handler(BaseHTTPRequestHandler):
    def log_message(self,*args): pass
    def do_GET(self):
        if self.path == '/cors-page':
            body=('<html><p id=cors>Waiting</p><script>document.cookie="preflight_fixture=1";fetch('+json.dumps(self.server.cors_target) + ',{method:"POST",credentials:"include",headers:{"Content-Type":"application/json"},body:"{}"}).then(r=>r.text()).then(t=>document.querySelector("#cors").textContent=t).catch(()=>document.querySelector("#cors").textContent="failed")</script></html>').encode();content='text/html'
        elif self.path == '/large-text':
            body=b'<html><body><pre id=large>'+b'x'*70000+b'</pre><button>Still alive</button></body></html>'; content='text/html'
        elif self.path == '/empty-aria-label':
            body=b'''<html><body><button aria-label="" onclick="document.querySelector('output').textContent='clicked'">Search flights</button>
                <button aria-label="   ">Whitespace label</button><button aria-label="Explicit name">Other text</button><output>Waiting</output></body></html>'''; content='text/html'
        elif self.path == '/binary':
            body=bytes(range(256))*150; content='application/octet-stream'
        else:
            body=HTML.encode();content='text/html; charset=utf-8'
        self.send_response(200);self.send_header('Content-Type',content)
        self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    def do_POST(self):
        self.server.posts.append(self.rfile.read(int(self.headers.get('Content-Length',0))))
        body=b'{"result":"Confirmed"}'
        self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)))
        self.end_headers();self.wfile.write(body)

class RuntimeBase(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        configured=os.environ.get('OBSCURA_RUNTIME_BIN')
        if not configured: self.skipTest('OBSCURA_RUNTIME_BIN required')
        self.binary=Path(configured)
        self.server=ThreadingHTTPServer(('127.0.0.1',0),Handler);self.server.posts=[]
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.origin=f'http://127.0.0.1:{self.server.server_port}'
        self.workspace=tempfile.TemporaryDirectory()
        self.browser=await Browser.launch({'binary':str(self.binary),'sha256':file_hash(self.binary)},
            self.workspace.name,PERSONA,[self.origin],initial_mode='RUNNING')
        self.page=await self.browser.new_page()
        await self.page.goto(self.origin)

    async def asyncTearDown(self):
        if hasattr(self,'browser'): await self.browser.close()
        if hasattr(self,'server'):
            await asyncio.to_thread(self.server.shutdown);self.server.server_close();self.thread.join()
            self.workspace.cleanup()

class RuntimeTests(RuntimeBase):
    async def test_empty_aria_label_falls_back_to_button_text(self):
        await self.page.goto(self.origin+'/empty-aria-label')
        button=self.page.get_by_role('button',name='Search flights',exact=True)
        self.assertEqual(await button.count(),1)
        self.assertEqual(await self.page.get_by_role('button',name='Whitespace label',exact=True).count(),1)
        self.assertEqual(await self.page.get_by_role('button',name='Explicit name',exact=True).count(),1)
        self.assertEqual(await self.page.get_by_role('button',name='Other text',exact=True).count(),0)
        await button.click()
        await expect(self.page.locator('output')).to_have_text('clicked')

    async def test_large_text_limit_keeps_session_usable(self):
        await self.page.goto(self.origin+'/large-text')
        with self.assertRaises(BrowserError) as error:
            await self.page.locator('#large').text_content()
        self.assertEqual(error.exception.code, 'VALUE_LIMIT')
        await expect(self.page.locator('#large')).to_contain_text('xxx')
        self.assertEqual(await self.page.get_by_role('button', name='Still alive').count(), 1)

    async def test_native_input_replacement_response_and_request_body(self):
        await self.page.get_by_label('Name',exact=True).fill('SDK payload')
        async with self.page.expect_response('**/api') as pending:
            await self.page.get_by_role('button',name='Send',exact=True).click()
        response=await pending.value
        self.assertEqual(await response.json(),{'result':'Confirmed'})
        self.assertEqual(await response.request.body(),b'SDK payload')
        self.assertEqual(self.server.posts,[b'SDK payload'])
        await expect(self.page.locator('#result')).to_have_text('Confirmed')
        self.assertEqual(await self.page.locator('#events').text_content(),'pointerdown,mousedown,pointerup,mouseup,click')

    async def test_wait_timeout_is_recoverable_and_queries_are_strict(self):
        with self.assertRaises(TimeoutError):
            await self.page.get_by_text('Missing',exact=True).click(timeout=350)
        await self.page.get_by_label('Name',exact=True).fill('Still alive')
        await expect(self.page.get_by_label('Name',exact=True)).to_have_value('Still alive')
        with self.assertRaises(BrowserError) as error:
            await self.page.get_by_role('button',name='Duplicate',exact=True).click()
        self.assertEqual(error.exception.code,'ELEMENT_AMBIGUOUS')
        self.assertEqual(await self.page.get_by_role('button',name='Duplicate',exact=True).count(),2)
        await self.page.get_by_text('Never present',exact=True).wait_for(state='hidden')

    async def test_scroll_select_check_and_close(self):
        await self.page.get_by_label('Currency').select_option('USD')
        await self.page.get_by_label('Terms',exact=True).check()
        await self.page.get_by_label('Terms',exact=True).uncheck()
        await self.page.get_by_role('button',name='Bottom',exact=True).click()
        await expect(self.page.locator('#result')).to_have_text('Bottom clicked')
        await self.page.close()
        with self.assertRaises(BrowserError): await self.page.locator('body').count()

    async def test_navigation_binary_chunking_and_page_isolation(self):
        other=await self.browser.new_page()
        observed=[];other.on('response',observed.append)
        async with self.page.expect_response('**/binary') as pending:
            await self.page.goto(self.origin+'/binary')
        self.assertEqual(await (await pending.value).body(),bytes(range(256))*150)
        self.assertEqual(observed,[])
        await other.close()


# The server also exposes independent dynamic cases for cancellation and callbacks.
CONDITIONS = '''<!doctype html><body>
<button id="start">Start</button><button id="target" style="position:absolute;left:20px;top:100px;width:120px;height:40px">Target</button>
<div id="cover" style="position:absolute;left:0;top:80px;width:300px;height:100px;background:red;z-index:2"></div>
<div id="result">Waiting</div><a id="link" href="/">Navigate</a>
<script>
window.ready=false;
document.querySelector('#start').onclick=()=>{
 setTimeout(()=>{fetch('/api',{method:'POST',body:'callback'});},50);
 setTimeout(()=>{document.querySelector('#cover').remove();window.ready=true},300);
};
document.querySelector('#target').onclick=()=>{document.querySelector('#result').textContent=window.ready?'Ready':'Too early'};
</script>'''
_old_get=Handler.do_GET
def _get(self):
    if self.path=='/redirect':
        self.send_response(302);self.send_header('Location','/conditions');self.send_header('Content-Length','0');self.end_headers()
    elif self.path=='/resource':
        body=b'<script src="/asset.js"></script><h1>Resource page</h1>'
        self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    elif self.path=='/asset.js':
        body=b'window.assetLoaded=true;'
        self.send_response(200);self.send_header('Content-Type','text/javascript');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    elif self.path=='/conditions':
        body=CONDITIONS.encode();self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    elif self.path.startswith('/storage'):
        body=b'<button onclick="localStorage.setItem(\'consent\',\'agreed\');sessionStorage.setItem(\'tab\',\'one\')">Agree</button><p id=stored></p><script>document.querySelector(\"#stored\").textContent=JSON.stringify([localStorage.getItem(\"consent\"),sessionStorage.getItem(\"tab\")])</script>'
        self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    elif self.path=='/hanging-task':
        body=b'<button onclick="setTimeout(()=>{while(true){}},100)">Start task</button>'
        self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    elif self.path=='/slow-task':
        body=b'<button onclick="setTimeout(()=>{const end=Date.now()+6000;while(Date.now()<end){};document.querySelector(\'#result\').textContent=\'Completed\'},100)">Start task</button><p id=result>Waiting</p>'
        self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    elif self.path=='/pseudo-hit':
        body=b'<style>.box{position:relative;width:200px;height:80px}.box::before{content:"";position:absolute;inset:0}#passive::before{pointer-events:none}#hidden::before{visibility:hidden}#opaque::before{opacity:0}</style><div class=box id=passive><button onclick="document.querySelector(\'#result\').textContent+=\'A\'">Passive</button></div><div class=box id=hidden><button onclick="document.querySelector(\'#result\').textContent+=\'B\'">Hidden</button></div><div class=box id=opaque><button onclick="document.querySelector(\'#result\').textContent+=\'WRONG\'">Covered</button></div><p id=result></p>'
        self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    elif self.path=='/transform-click':
        body=b'<button id=target style="transform:scale(.8)" onclick="this.textContent=\'Completed\'">Transform</button><script>setTimeout(()=>document.querySelector(\"#target\").style.transform=\"none\",500)</script>'
        self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    elif self.path=='/slow-click':
        body=b'<button onclick="const end=Date.now()+1200;while(Date.now()<end){};this.textContent=\'Completed\'">Slow</button>'
        self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    elif self.path=='/disable-on-click':
        body=b'<button onclick="this.disabled=true;document.querySelector(\'#result\').textContent=\'Completed\'">Next</button><p id=result>Pending</p>'
        self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    elif self.path=='/hang':
        body=b'<button onclick="while(true){}">Hang</button>'
        self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    else:_old_get(self)
Handler.do_GET=_get

class WaitingTests(RuntimeBase):
    async def test_overlay_wait_keeps_network_callback_running(self):
        await self.page.goto(self.origin+'/conditions')
        result=asyncio.get_running_loop().create_future()
        async def response(r):
            if r.url.endswith('/api'): result.set_result(await r.json())
        self.page.on('response',response)
        await self.page.get_by_role('button',name='Start',exact=True).click()
        await self.page.get_by_role('button',name='Target',exact=True).click()
        await expect(self.page.locator('#result')).to_have_text('Ready')
        self.assertEqual(await asyncio.wait_for(result,2),{'result':'Confirmed'})
        self.page.off('response',response)
        self.assertEqual(self.page.callback_errors,[])

    async def test_navigation_updates_generation(self):
        await self.page.goto(self.origin+'/conditions')
        before=self.page.ref.page_generation
        await self.page.get_by_role('link',name='Navigate',exact=True).click()
        self.assertGreater(self.page.ref.page_generation,before)
        await self.page.get_by_label('Name',exact=True).fill('After navigation')

    async def test_cancel_wait_kills_owned_process(self):
        task=asyncio.create_task(self.page.locator('#missing').click())
        await asyncio.sleep(.1)
        task.cancel()
        with self.assertRaises(asyncio.CancelledError):await task
        await asyncio.wait_for(self.browser.session.process.wait(),3)

    async def test_watchdog_stops_synchronous_click_handler(self):
        await self.page.goto(self.origin+'/hang')
        with self.assertRaises(BrowserError) as error:
            await self.page.get_by_role('button',name='Hang',exact=True).click(timeout=2000)
        self.assertIn(error.exception.dispatch_state,{'SENT','UNKNOWN'})
        await asyncio.wait_for(self.browser.session.process.wait(),3)

    async def test_storage_survives_navigation_and_separates_tabs(self):
        await self.page.goto(self.origin+'/storage')
        await self.page.get_by_role('button',name='Agree',exact=True).click()
        await self.page.goto(self.origin+'/storage?next')
        await expect(self.page.locator('#stored')).to_have_text('[\"agreed\",\"one\"]',timeout=2000)
        other=await self.browser.new_page()
        await other.goto(self.origin+'/storage')
        await expect(other.locator('#stored')).to_have_text('[\"agreed\",null]',timeout=2000)

    async def test_wait_terminates_hung_page_task(self):
        await self.page.goto(self.origin+'/hanging-task')
        await self.page.get_by_role('button',name='Start task',exact=True).click()
        with self.assertRaises(BrowserError) as error:
            await self.page.locator('#missing').wait_for(timeout=1000)
        self.assertEqual(error.exception.code,'INPUT_TIMEOUT')
        await asyncio.wait_for(self.browser.session.process.wait(),3)

    async def test_wait_keeps_long_synchronous_page_task_alive(self):
        await self.page.goto(self.origin+'/slow-task')
        await self.page.get_by_role('button',name='Start task',exact=True).click(timeout=12000)
        await expect(self.page.locator('#result')).to_have_text('Completed',timeout=12000)

    async def test_idle_condition_timeout_does_not_fire_task_watchdog(self):
        for _ in range(10):
            with self.assertRaises(TimeoutError) as error:
                await self.page.locator('#missing').wait_for(timeout=50)
            self.assertEqual(error.exception.dispatch_state,'NOT_SENT')
        await self.page.get_by_label('Name',exact=True).fill('Still usable')

    async def test_click_can_disable_its_own_button(self):
        await self.page.goto(self.origin+'/disable-on-click')
        await self.page.get_by_role('button',name='Next',exact=True).click()
        await expect(self.page.locator('#result')).to_have_text('Completed')

    async def test_idle_page_keeps_long_synchronous_task_alive(self):
        await self.page.goto(self.origin+'/slow-task')
        await self.page.get_by_role('button',name='Start task',exact=True).click(timeout=12000)
        await asyncio.sleep(7)
        await expect(self.page.locator('#result')).to_have_text('Completed',timeout=2000)

    async def test_body_read_survives_long_synchronous_page_task(self):
        async with self.page.expect_response('**/slow-task') as pending:
            await self.page.goto(self.origin+'/slow-task')
        response=await pending.value
        async def read_during_task():
            await asyncio.sleep(.5)
            return await response.body()
        read=asyncio.create_task(read_during_task())
        try:
            await self.page.get_by_role('button',name='Start task',exact=True).click(timeout=12000)
            await expect(self.page.locator('#result')).to_have_text('Completed',timeout=12000)
            self.assertIn(b'Start task',await read)
        finally:
            await asyncio.gather(read,return_exceptions=True)

    async def test_non_hit_testable_pseudos_do_not_block_clicks(self):
        await self.page.goto(self.origin+'/pseudo-hit')
        await self.page.get_by_role('button',name='Passive',exact=True).click(timeout=1000)
        await self.page.get_by_role('button',name='Hidden',exact=True).click(timeout=1000)
        with self.assertRaises(BrowserError):
            await self.page.get_by_role('button',name='Covered',exact=True).click(timeout=1000)
        await expect(self.page.locator('#result')).to_have_text('AB')

    async def test_click_waits_for_transformed_geometry_to_settle(self):
        await self.page.goto(self.origin+'/transform-click')
        await self.page.get_by_role('button',name='Transform',exact=True).click(timeout=4000)
        await expect(self.page.get_by_role('button')).to_have_text('Completed')

    async def test_synchronous_click_uses_action_deadline(self):
        await self.page.goto(self.origin+'/slow-click')
        await self.page.get_by_role('button',name='Slow',exact=True).click(timeout=4000)
        await expect(self.page.get_by_role('button')).to_have_text('Completed')

    async def test_redirect_chain_and_script_body(self):
        async with self.page.expect_response('**/conditions') as pending:
            await self.page.goto(self.origin+'/redirect')
        response=await pending.value
        self.assertEqual(response.redirected_from,[self.origin+'/redirect'])
        async with self.page.expect_response('**/asset.js') as script:
            await self.page.goto(self.origin+'/resource')
        self.assertEqual(await (await script.value).text(),'window.assetLoaded=true;')

    async def test_response_timeout_does_not_suggest_replaying_click(self):
        with self.assertRaises(TimeoutError) as error:
            async with self.page.expect_response('**/never',timeout=250):
                await self.page.get_by_role('button',name='Bottom',exact=True).click()
        self.assertEqual(error.exception.dispatch_state,'SENT')
        await expect(self.page.locator('#result')).to_have_text('Bottom clicked')

    async def test_paused_page_can_be_closed(self):
        await self.browser.session.set_mode(1,'PAUSED')
        await self.page.close()
        self.assertTrue(self.page.closed)

if __name__=='__main__': unittest.main()

_identity_get = Handler.do_GET
def _identity_handler(self):
    if self.path == '/identity':
        self.server.identity_headers = dict(self.headers.items())
        body = b'''<!doctype html><pre id="identity">Waiting</pre><script src="/identity-script.js"></script><script>
        navigator.userAgentData.getHighEntropyValues(['architecture','bitness','fullVersionList','model','platformVersion','uaFullVersion','wow64']).then(high=>{
          document.querySelector('#identity').textContent=JSON.stringify({ua:navigator.userAgent,platform:navigator.platform,language:navigator.language,languages:navigator.languages,high});
        });</script>'''
        self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    elif self.path == '/identity-script.js':
        self.server.identity_script_headers = dict(self.headers.items())
        body=b'window.identityScriptLoaded=true;'
        self.send_response(200);self.send_header('Content-Type','application/javascript');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    else:
        _identity_get(self)
Handler.do_GET = _identity_handler

class CorsHandler(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_OPTIONS(self):
        self.server.preflight_headers = dict(self.headers.items())
        self.send_response(204)
        self.send_header('Access-Control-Allow-Origin', self.headers.get('Origin',''))
        self.send_header('Access-Control-Allow-Credentials', 'true')
        if self.path == '/denied':
            self.send_header('Access-Control-Allow-Origin', 'https://other.invalid')
        self.send_header('Access-Control-Allow-Headers', 'content-type')
        self.send_header('Access-Control-Allow-Methods', 'POST')
        self.end_headers()
    def do_POST(self):
        self.rfile.read(int(self.headers.get('Content-Length', 0)))
        self.server.post_count=getattr(self.server,'post_count',0)+1
        self.server.post_headers=dict(self.headers.items())
        self.send_response(200);self.send_header('Access-Control-Allow-Origin', self.headers.get('Origin',''))
        self.send_header('Access-Control-Allow-Credentials', 'true')
        self.send_header('Content-Length', '2');self.end_headers();self.wfile.write(b'ok')

class IdentityTests(RuntimeBase):
    async def test_preflight_uses_selected_stealth_transport(self):
        server=ThreadingHTTPServer(('127.0.0.1',0),CorsHandler)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        target=f'http://127.0.0.1:{server.server_port}'
        self.server.cors_target=target
        try:
            with tempfile.TemporaryDirectory() as workspace:
                persona={**PERSONA, 'profile':'macos_chrome152'}
                async with await Browser.launch({'binary':str(self.binary),'sha256':file_hash(self.binary)},workspace,persona,[self.origin,target],initial_mode='RUNNING') as browser:
                    page=await browser.new_page();await page.goto(self.origin+'/cors-page')
                    await expect(page.locator('#cors')).to_have_text('ok')
                    headers={k.lower():v for k,v in server.preflight_headers.items()}
                    self.assertFalse(any(key.startswith('sec-ch-') for key in headers))
                    self.assertIn('Chrome/152.0.0.0', headers.get('user-agent',''))
                    self.assertNotIn('cookie', headers)
                    post={k.lower():v for k,v in server.post_headers.items()}
                    self.assertIn('preflight_fixture=1', post.get('cookie',''))
                    self.assertEqual(post.get('sec-ch-ua-platform'), '"macOS"')
                    self.server.cors_target=target+'/denied'
                    await page.goto(self.origin+'/cors-page')
                    await expect(page.locator('#cors')).to_have_text('failed')
                    self.assertEqual(server.post_count,1)
        finally:
            await asyncio.to_thread(server.shutdown);server.server_close();thread.join()

    async def test_macos_chrome152_identity_on_wire_and_in_page(self):
        persona = {k:v for k,v in PERSONA.items() if k != 'profile'}
        with tempfile.TemporaryDirectory() as workspace:
            binary = os.environ['OBSCURA_RUNTIME_BIN']
            async with await Browser.launch({'binary':binary,'sha256':file_hash(binary)},workspace,persona,[self.origin],initial_mode='RUNNING') as browser:
                self.assertEqual(browser.identity['profile'], 'macos_chrome152')
                self.assertEqual(browser.identity['transport_profile'], 'primp_chrome152_macos')
                self.assertFalse(browser.identity['chrome152_transport_verified'])
                page = await browser.new_page()
                await page.goto(self.origin+'/identity')
                await expect(page.locator('#identity')).to_contain_text('uaFullVersion')
                actual = json.loads(await page.locator('#identity').text_content())
                headers = {k.lower():v for k,v in self.server.identity_headers.items()}
                self.assertEqual(actual['ua'], 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36')
                self.assertEqual(headers['user-agent'], actual['ua'])
                self.assertEqual(headers['sec-ch-ua-platform'], '"macOS"')
                self.assertEqual(headers['sec-ch-ua'], '"Chromium";v="152", "Not?A_Brand";v="24", "Google Chrome";v="152"')
                self.assertEqual(headers['accept-language'], 'en,zh-CN;q=0.9,zh;q=0.8')
                self.assertNotIn('sec-purpose', headers)
                script = {k.lower():v for k,v in self.server.identity_script_headers.items()}
                self.assertEqual(script['user-agent'], headers['user-agent'])
                self.assertEqual(script['sec-ch-ua'], headers['sec-ch-ua'])
                self.assertEqual(script['accept-encoding'], 'gzip, deflate, br, zstd')
                self.assertEqual(script['priority'], 'u=0, i')
                self.assertEqual(script['sec-fetch-dest'], 'script')
                self.assertNotIn('sec-fetch-user', script)
                self.assertNotIn('upgrade-insecure-requests', script)
                order = ['sec-ch-ua','sec-ch-ua-mobile','sec-ch-ua-platform','upgrade-insecure-requests','user-agent','accept','sec-fetch-site','sec-fetch-mode','sec-fetch-user','sec-fetch-dest','accept-encoding','accept-language','priority']
                self.assertEqual([name for name in headers if name in order], order)
                self.assertEqual(actual['platform'], 'MacIntel')
                self.assertEqual(actual['language'], 'en')
                self.assertEqual(actual['languages'], ['en','zh-CN'])
                high = actual['high']
                self.assertEqual(high['architecture'], 'arm')
                self.assertEqual(high['platformVersion'], '26.6.2')
                self.assertEqual(high['uaFullVersion'], '152.0.7977.83')
                self.assertEqual(high['fullVersionList'], [{'brand':'Chromium','version':'152.0.7977.83'},{'brand':'Not?A_Brand','version':'24.0.0.0'},{'brand':'Google Chrome','version':'152.0.7977.83'}])
