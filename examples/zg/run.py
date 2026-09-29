"""Run the ZG checkout to its payment choice with official Playwright Python."""
import argparse
import asyncio
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from urllib.request import urlopen

from playwright.async_api import async_playwright

from fixture import CONFIG, Fixture
from flow import BookingError, checkout, validate


def faker_japan_config(departure_date):
    from faker import Faker
    faker = Faker('ja_JP')
    gender = faker.random_element(['M', 'F'])
    first = faker.first_romanized_name_male() if gender == 'M' else faker.first_romanized_name_female()
    last = faker.last_romanized_name()
    return {
        'origin': '吉隆坡 KUL', 'destination': '东京 NRT',
        'departure_date': departure_date, 'currency': 'USD - US$',
        'passenger': {
            'first_name': first.upper(), 'last_name': last.upper(),
            'birthday': faker.date_of_birth(minimum_age=25, maximum_age=45).isoformat(),
            'gender': gender, 'nationality_label': '日本',
            'email': faker.user_name() + '@example.invalid',
            'phone': ''.join(char for char in faker.phone_number() if char.isdigit()),
            'phone_code': '+81 (日本)', 'passport': faker.bothify('??#######').upper(),
            'passport_expiry': '2031-12-31',
        },
    }


def unused_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


async def wait_for_cdp(port, process):
    endpoint = f'http://127.0.0.1:{port}'
    for _ in range(100):
        if process.poll() is not None:
            raise BookingError('BROWSER_EXITED')
        try:
            response = await asyncio.to_thread(urlopen, endpoint + '/json/version', None, 0.5)
            response.close()
            return endpoint
        except OSError:
            await asyncio.sleep(0.1)
    raise BookingError('BROWSER_START_TIMEOUT')


async def execute(args):
    stages = []
    started = time.monotonic()
    def stage(name):
        stages.append({'stage': name, 'elapsed_seconds': round(time.monotonic() - started, 3)})
        print('ZG stage: ' + name, file=sys.stderr, flush=True)

    mode = 'live' if args.live else 'fixture'
    fixture = None
    process = None
    try:
        config = (json.loads(Path(args.config).read_text()) if args.config else
                  faker_japan_config(args.departure_date) if args.live and args.faker_jp and args.departure_date else
                  dict(CONFIG))
        if args.live and not (args.config or args.faker_jp and args.departure_date):
            raise BookingError('LIVE_CONFIG_REQUIRED')
        validate(config)
        login = {'account': 'fixture', 'password': 'fixture'}
        if args.live:
            config['base_url'] = 'https://www.zipair.net/zh-cn/'
        else:
            fixture = Fixture(args.scenario).__enter__()
            config['base_url'] = fixture.origin

        port = unused_port()
        command = [str(Path(args.binary).resolve()), 'serve', '--host', '127.0.0.1',
                   '--persona', args.persona or ('macos_chrome153' if args.live else 'windows_chrome145'),
                   '--port', str(port)]
        if fixture:
            command.append('--allow-private-network')
        if args.proxy:
            command.extend(['--proxy', args.proxy])
        with tempfile.TemporaryFile() as server_log:
            process = subprocess.Popen(command, stdout=server_log, stderr=server_log)
            endpoint = await wait_for_cdp(port, process)
            async with async_playwright() as playwright:
                browser = await playwright.chromium.connect_over_cdp(endpoint)
                try:
                    context = await browser.new_context(**({'device_scale_factor': 2} if args.live else {}))
                    page = await context.new_page()
                    result = await checkout(page, config, login, fixture=bool(fixture), stage=stage)
                finally:
                    await browser.close()
        if fixture and fixture.payments:
            raise BookingError('PAYMENT_REQUEST_SENT')
        return {**result, 'stages': stages, 'mode': mode,
                **({'payment_requests': len(fixture.payments)} if fixture else {})}
    except Exception as error:
        # Never print raw Playwright errors, URLs, credentials, or passenger values.
        return {'status': 'BLOCKED' if args.live else 'FAILED',
                'stage': stages[-1]['stage'] if stages else 'init',
                'error': str(error) if isinstance(error, BookingError) else type(error).__name__,
                **({'detail': str(error)} if fixture else {}),
                'stages': stages, 'mode': mode,
                **({'payment_requests': len(fixture.payments)} if fixture else {})}
    finally:
        if process:
            process.terminate()
            try:
                await asyncio.to_thread(process.wait, 5)
            except subprocess.TimeoutExpired:
                process.kill()
                await asyncio.to_thread(process.wait)
        if fixture:
            await asyncio.to_thread(fixture.__exit__, None, None, None)


def main():
    parser = argparse.ArgumentParser(description='ZG checkout through payment review')
    parser.add_argument('--binary', default='target/release/obscura')
    parser.add_argument('--persona')
    parser.add_argument('--live', action='store_true')
    parser.add_argument('--config', help='private JSON itinerary and authorized passenger data')
    parser.add_argument('--faker-jp', action='store_true', help='generate a Japanese test passenger for KUL-NRT')
    parser.add_argument('--departure-date', help='YYYY-MM-DD, required with --faker-jp')
    parser.add_argument('--proxy')
    parser.add_argument('--scenario', choices=['success', 'no_flight', 'wrong_flight', 'price_change'], default='success')
    args = parser.parse_args()
    result = asyncio.run(execute(args))
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0 if result['status'] == 'READY_FOR_PAYMENT' else 1


if __name__ == '__main__':
    raise SystemExit(main())
