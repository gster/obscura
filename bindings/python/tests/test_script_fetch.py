"""Dynamic classic scripts use CORS settings and the document origin."""
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import tempfile
import threading
import unittest
from urllib.parse import parse_qs, urlsplit
from obscura_runtime import Browser, expect, file_hash

class ScriptHandler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        query = parse_qs(urlsplit(self.path).query)
        if self.path.startswith('/script.js'):
            name = query['case'][0]
            body = ('window.received = ' + json.dumps({
                'cookie': 'script_session=fixture' in self.headers.get('Cookie', ''),
                'origin': self.headers.get('Origin'),
                'destination': self.headers.get('Sec-Fetch-Dest'),
            }) + ';').encode()
            self.send_response(200)
            self.send_header('Content-Type', 'text/javascript')
            if name != 'denied' and self.headers.get('Origin'):
                self.send_header('Access-Control-Allow-Origin', self.headers['Origin'])
                self.send_header('Access-Control-Allow-Credentials', 'true')
        else:
            target = self.server.script_origin
            body = ('''<!doctype html><html><head></head><body><pre id=result>pending</pre>
<script>
(async () => {
 document.cookie='script_session=fixture; Path=/';
 const results=[];
 for(const [name,attr] of [['default',null],['anonymous','anonymous'],['credentials','use-credentials'],['denied','anonymous'],['base','anonymous']]) {
  if(name==='base') {const base=document.createElement('base');base.href=TARGET+'/';document.head.appendChild(base);}
  window.received=null;
  const script=document.createElement('script');
  if(attr!==null) script.setAttribute('crossorigin',attr);
  script.src=name==='base' ? 'script.js?case=base' : TARGET+'/script.js?case='+name;
  const event=await new Promise(resolve=>{script.onload=()=>resolve('load');script.onerror=()=>resolve('error');document.head.appendChild(script)});
  results.push({name,event,data:window.received});
 }
 document.querySelector('#result').textContent=JSON.stringify(results);
})().catch(e=>document.querySelector('#result').textContent='ERROR:'+e.message);
</script></body></html>'''.replace('TARGET', json.dumps(target))).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

class ScriptServers:
    def __enter__(self):
        self.servers = [ThreadingHTTPServer(('127.0.0.1', 0), ScriptHandler) for _ in range(2)]
        self.origins = [f'http://127.0.0.1:{s.server_port}' for s in self.servers]
        self.threads = []
        for server in self.servers:
            server.script_origin = self.origins[1]
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            self.threads.append(thread)
        return self

    def __exit__(self, *args):
        for server, thread in zip(self.servers, self.threads):
            server.shutdown()
            server.server_close()
            thread.join()

class DynamicScriptTests(unittest.IsolatedAsyncioTestCase):
    async def test_crossorigin_controls_credentials_and_execution(self):
        binary = os.environ.get('OBSCURA_RUNTIME_BIN')
        if not binary:
            self.skipTest('OBSCURA_RUNTIME_BIN required')
        with ScriptServers() as servers, tempfile.TemporaryDirectory() as workspace:
            async with await Browser.launch(
                {'binary':binary, 'sha256':file_hash(Path(binary))}, workspace,
                {'schema_version':'1','persona_id':'script_fetch','revision':'1',
                 'profile':'macos_chrome152','viewport':{'width':800,'height':600}},
                servers.origins, initial_mode='RUNNING',
            ) as browser:
                page = await browser.new_page()
                await page.goto(servers.origins[0])
                await expect(page.locator('#result')).to_contain_text('"name":"base"')
                result = json.loads(await page.locator('#result').text_content())
                expected = [
                    {'name':'default','event':'load','data':{'cookie':True,'origin':None,'destination':'script'}},
                    {'name':'anonymous','event':'load','data':{'cookie':False,'origin':servers.origins[0],'destination':'script'}},
                    {'name':'credentials','event':'load','data':{'cookie':True,'origin':servers.origins[0],'destination':'script'}},
                    {'name':'denied','event':'error','data':None},
                    {'name':'base','event':'load','data':{'cookie':False,'origin':servers.origins[0],'destination':'script'}},
                ]
                self.assertEqual(result, expected)

if __name__ == '__main__':
    unittest.main()
