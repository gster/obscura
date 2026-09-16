"""Run the same supported locator/input sequence with Chromium and Obscura."""
import asyncio
from http.server import ThreadingHTTPServer
import json
import os
from pathlib import Path
import tempfile
import threading
from playwright.async_api import async_playwright
from obscura_runtime import Browser, file_hash
from test_automation import Handler, PERSONA

async def sequence(page,origin):
    await page.goto(origin)
    await page.get_by_label('Name',exact=True).fill('comparison')
    await page.get_by_label('Currency').select_option('USD')
    await page.get_by_label('Terms',exact=True).check()
    async with page.expect_response('**/api') as pending:
        await page.get_by_role('button',name='Send',exact=True).click()
    response=await pending.value
    return {'response':await response.json(),'name':await page.get_by_label('Name',exact=True).input_value(),
            'duplicates':await page.get_by_role('button',name='Duplicate',exact=True).count(),
            'events':await page.locator('#events').text_content()}

async def main():
    server=ThreadingHTTPServer(('127.0.0.1',0),Handler);server.posts=[]
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
    origin=f'http://127.0.0.1:{server.server_port}'
    try:
        async with async_playwright() as pw:
            chrome=await pw.chromium.launch(headless=True, **({"executable_path":os.environ["CHROMIUM_BIN"]} if "CHROMIUM_BIN" in os.environ else {}))
            try:expected=await sequence(await chrome.new_page(),origin)
            finally:await chrome.close()
        binary=os.environ['OBSCURA_RUNTIME_BIN']
        with tempfile.TemporaryDirectory() as workspace:
            async with await Browser.launch({'binary':binary,'sha256':file_hash(binary)},workspace,PERSONA,[origin],initial_mode='RUNNING') as browser:
                actual=await sequence(await browser.new_page(),origin)
        assert actual==expected,(actual,expected)
        print(json.dumps({'status':'MATCH','result':actual}))
    finally:
        server.shutdown();server.server_close();thread.join()
if __name__=='__main__':asyncio.run(main())
