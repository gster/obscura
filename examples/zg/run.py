"""python examples/zg/run.py --help"""
import argparse
import ast
import asyncio
import json
from pathlib import Path
import tempfile
import time
import sys
from obscura_runtime import Browser, file_hash
from flow import checkout, validate, BookingError
from fixture import Fixture, CONFIG


def credentials(path):
    # Parse literal assignments only. Importing this file could execute business code.
    found={}
    for node in ast.parse(Path(path).read_text()).body:
        if isinstance(node,ast.Assign):
            for target in node.targets:
                names = [target] if isinstance(target, ast.Name) else target.elts if isinstance(target, (ast.Tuple, ast.List)) else []
                if not all(isinstance(name, ast.Name) for name in names): continue
                if not any(name.id in {'ZG_ACCOUNT', 'ZG_PASSWORD'} for name in names): continue
                try: value = ast.literal_eval(node.value)
                except (ValueError, TypeError): raise BookingError('CREDENTIALS_UNAVAILABLE') from None
                values = [value] if isinstance(target, ast.Name) else value
                if not isinstance(values, (tuple, list)) or len(names) != len(values):
                    raise BookingError('CREDENTIALS_UNAVAILABLE')
                for name, value in zip(names, values):
                    if name.id in {'ZG_ACCOUNT', 'ZG_PASSWORD'}: found[name.id] = value
    if not all(isinstance(found.get(k),str) and found[k] for k in ('ZG_ACCOUNT','ZG_PASSWORD')):
        raise BookingError('CREDENTIALS_UNAVAILABLE')
    return {'account':found['ZG_ACCOUNT'],'password':found['ZG_PASSWORD']}


async def execute(args):
    stages=[];start=time.monotonic()
    def stage(name):
        stages.append({'stage':name,'elapsed_seconds':round(time.monotonic()-start,3)})
        print('ZG stage: '+name,file=sys.stderr,flush=True)
    config=json.loads(Path(args.config).read_text()) if args.config else dict(CONFIG)
    if args.live and not args.config:raise BookingError('LIVE_CONFIG_REQUIRED')
    validate(config)
    login=credentials(args.credentials) if args.live else {'account':'test@example.invalid','password':'fixture'}
    fixture=None
    if not args.live:
        fixture=Fixture(args.scenario).__enter__();config['base_url']=fixture.origin
        origins=[fixture.origin]
    else:
        config['base_url']='https://www.zipair.net/zh-cn/'
        origins=config.get('allowed_origins',['https://www.zipair.net','https://bff.zipair.net','https://images.zipair.net',
            'https://consols.zipair.net','https://hydra.zipair.net','https://idp-app.zipair.net',
            'https://idp-approval.zipair.net','https://zipair-idp.s3-ap-northeast-1.amazonaws.com'])
    spec={'binary':str(Path(args.binary).resolve()),'sha256':file_hash(args.binary)}
    if args.proxy:spec['proxy_url']=args.proxy
    persona={'schema_version':'1','persona_id':'zg_sdk','revision':'1','profile':'macos_chrome152','viewport':{'width':1280,'height':900}}
    try:
        with tempfile.TemporaryDirectory(prefix='zg-sdk-') as workspace:
            async with await Browser.launch(spec,workspace,persona,origins,initial_mode='RUNNING') as browser:
                page=await browser.new_page()
                result=await checkout(page,config,login,offline=not args.live,stage=stage)
                return {**result,'stages':stages,'mode':'live' if args.live else 'offline'}
    except Exception as error:
        # Never log raw exceptions containing URLs, credentials or passenger values.
        return {'status':'BLOCKED' if args.live else 'FAILED','stage':stages[-1]['stage'] if stages else 'init',
                'error':getattr(error,'code',str(error) if isinstance(error,BookingError) else type(error).__name__),
                'stages':stages,'mode':'live' if args.live else 'offline'}
    finally:
        if fixture:fixture.__exit__()


def main():
    parser=argparse.ArgumentParser(description='ZG offline checkout or live payment-before verification')
    parser.add_argument('--binary',required=True)
    parser.add_argument('--live',action='store_true')
    parser.add_argument('--config',help='private JSON itinerary and authorized passenger data')
    parser.add_argument('--credentials',default='/Users/zg/work/autopilot/routers/zg/config.py')
    parser.add_argument('--proxy')
    parser.add_argument('--scenario',choices=['success','no_flight','wrong_flight','price_change','payment_failure'],default='success')
    args=parser.parse_args()
    try:result=asyncio.run(execute(args))
    except BookingError as error:result={'status':'BLOCKED','error':str(error)}
    print(json.dumps(result,ensure_ascii=False,indent=2))
    return 0 if result['status'] in {'SIMULATED_BOOKED','READY_FOR_PAYMENT'} else 1

if __name__=='__main__':raise SystemExit(main())
