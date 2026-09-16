import asyncio
import copy
import os
from pathlib import Path
import sys
import tempfile
import unittest
from obscura_runtime import Browser, file_hash
sys.path.insert(0,str(Path(__file__).resolve().parents[3]/'examples'/'zg'))
from fixture import Fixture, CONFIG
from flow import checkout, BookingError
from test_automation import PERSONA

class ZGTests(unittest.IsolatedAsyncioTestCase):
    async def run_case(self,scenario,expected):
        binary=os.environ.get('OBSCURA_RUNTIME_BIN')
        if not binary:self.skipTest('OBSCURA_RUNTIME_BIN required')
        fixture=Fixture(scenario).__enter__()
        try:
            with tempfile.TemporaryDirectory() as workspace:
                async with await Browser.launch({'binary':binary,'sha256':file_hash(binary)},workspace,PERSONA,[fixture.origin],initial_mode='RUNNING') as browser:
                    page=await browser.new_page();config=copy.deepcopy(CONFIG);config['base_url']=fixture.origin
                    if expected=='SIMULATED_BOOKED':
                        result=await checkout(page,config,{'account':'fixture','password':'fixture'},offline=True)
                        self.assertEqual(result,{'status':'SIMULATED_BOOKED','pnr':'TESTPNR'})
                    else:
                        with self.assertRaises(BookingError) as error:
                            await checkout(page,config,{'account':'fixture','password':'fixture'},offline=True)
                        self.assertEqual(str(error.exception),expected)
                    self.assertEqual(len(fixture.payments),1 if scenario in {'success','payment_failure'} else 0)
        finally:await asyncio.to_thread(fixture.__exit__)
    async def test_success(self):await self.run_case('success','SIMULATED_BOOKED')
    async def test_no_flight(self):await self.run_case('no_flight','NO_FLIGHTS')
    async def test_wrong_flight(self):await self.run_case('wrong_flight','FLIGHT_MISMATCH')
    async def test_price_change(self):await self.run_case('price_change','PRICE_MISMATCH')
    async def test_payment_failure(self):await self.run_case('payment_failure','PAYMENT_FAILED')

class CredentialTests(unittest.TestCase):
    def test_literal_credentials_without_executing_legacy_module(self):
        from run import credentials
        with tempfile.TemporaryDirectory() as work:
            path=Path(work)/'config.py'
            for assignment in ["ZG_ACCOUNT='fixture'; ZG_PASSWORD='secret'", "ZG_ACCOUNT, ZG_PASSWORD=('fixture','secret')"]:
                path.write_text("raise RuntimeError('must not execute')\n"+assignment)
                self.assertEqual(credentials(path),{'account':'fixture','password':'secret'})
            path.write_text("ZG_ACCOUNT, ZG_PASSWORD = load_secrets()")
            with self.assertRaises(BookingError): credentials(path)
