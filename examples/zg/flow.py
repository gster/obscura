"""ZG single-adult one-way checkout. The live branch cannot submit payment."""
from datetime import datetime
from decimal import Decimal
import re
from obscura_runtime import expect, TimeoutError


class BookingError(Exception):
    pass


def validate(config):
    for key in ('origin','destination','departure_date','departure_time','flight_number','currency','amount','passenger'):
        if key not in config: raise BookingError('CONFIG_MISSING_'+key.upper())
    datetime.strptime(config['departure_date'],'%Y-%m-%d')
    if not re.fullmatch(r'ZG\d{3,4}',config['flight_number']): raise BookingError('INVALID_FLIGHT_NUMBER')
    if Decimal(str(config['amount']))<=0: raise BookingError('INVALID_AMOUNT')
    p=config['passenger']
    for key in ('first_name','last_name','birthday','gender','nationality_label','email','phone','phone_code','passport','passport_expiry'):
        if not p.get(key): raise BookingError('PASSENGER_MISSING_'+key.upper())
    dep=datetime.strptime(config['departure_date'],'%Y-%m-%d').date()
    birth=datetime.strptime(p['birthday'],'%Y-%m-%d').date()
    age=dep.year-birth.year-((dep.month,dep.day)<(birth.month,birth.day))
    if age<15: raise BookingError('ONLY_ADULT_SUPPORTED')
    if p['gender'] not in {'M','F'}: raise BookingError('INVALID_GENDER')
    if datetime.strptime(p['passport_expiry'],'%Y-%m-%d').date()<=dep: raise BookingError('PASSPORT_EXPIRED')
    if config.get('return_date') or config.get('baggage') or config.get('seats'): raise BookingError('UNSUPPORTED_ITINERARY')


async def optional(locator, timeout=1000):
    try: await locator.click(timeout=timeout)
    except TimeoutError as error:
        if error.dispatch_state!='NOT_SENT': raise


async def date_fields(page, group, date):
    year,month,day=date.split('-')
    root=page.get_by_role('group',name=group)
    await root.get_by_role('button',name='日',exact=True).click()
    await page.get_by_role('option',name=str(int(day)),exact=True).click()
    await root.get_by_role('button',name='月',exact=True).click()
    await page.get_by_role('option',name=datetime.strptime(month,'%m').strftime('%b'),exact=True).click()
    await root.get_by_role('button',name='年',exact=True).click()
    await page.get_by_role('option',name=year,exact=True).click()


async def checkout(page, config, credentials, *, offline=False, stage=lambda name:None):
    validate(config)
    if offline and not config['base_url'].startswith('http://127.0.0.1:'):
        raise BookingError('OFFLINE_REQUIRES_LOOPBACK')
    page.set_default_timeout(60000)
    stage('login')
    await page.goto(config.get('base_url','https://www.zipair.net/zh-cn/'))
    cookie_consent=page.get_by_role('button',name='同意',exact=True)
    await cookie_consent.click()
    await cookie_consent.wait_for(state='hidden')
    if not await page.get_by_role('link',name='登录',exact=True).is_visible():
        await page.get_by_role('button',name='打开菜单',exact=True).click()
    await page.get_by_role('link',name='登录',exact=True).click()
    await page.get_by_label('电子邮箱※',exact=True).fill(credentials['account'])
    await page.get_by_label('密码※',exact=True).fill(credentials['password'])
    await page.get_by_role('button',name='登录',exact=True).click()
    await page.get_by_role('button',name='点击并更改货币。',exact=True).click()
    await page.get_by_role('link',name=config['currency'],exact=True).click()
    stage('search')
    await page.get_by_role('tab',name='单程包括中转',exact=True).click()
    await page.get_by_role('button',name='出发地（国家，地区）',exact=True).click()
    await page.get_by_role('button',name=config['origin'],exact=True).click()
    await page.get_by_role('button',name='目的地（国家，地区）',exact=True).click()
    await page.get_by_role('button',name=config['destination'],exact=True).click()
    await page.get_by_text('搜索机票',exact=True).click()
    await page.get_by_role('button',name='下一步',exact=True).click()
    await page.get_by_role('button',name='下一步',exact=True).click()
    await page.locator(f'button[data-fulldate="{config["departure_date"]}"]').click()
    await page.get_by_alt_text('正在读取中',exact=True).wait_for(state='hidden',timeout=180000)
    if await page.get_by_text('没有航班',exact=True).is_visible(): raise BookingError('NO_FLIGHTS')
    await expect(page.locator('div.cabin-contents').first).to_be_visible(timeout=180000)
    selected=False
    for flight in await page.locator('div.cabin-contents').all():
        parts=[await item.text_content() for item in await flight.locator(config.get('flight_number_selector','span')).all()]
        start=(await flight.locator('span.start').text_content()).strip()
        numbers=[match.group(1) for text in parts if (match:=re.fullmatch(r'\s*ZG\s*(\d{1,4})\s*',text))]
        if any(int(n)==int(config['flight_number'][2:]) for n in numbers) and start==config['departure_time']:
            await optional(flight.get_by_text('选择座位类型',exact=True))
            await flight.get_by_text('座位类型Standard',exact=True).click()
            selected=True;break
    if not selected: raise BookingError('FLIGHT_MISMATCH')
    await page.get_by_role('button',name='下一步',exact=True).click()
    await page.get_by_role('link',name='不要买套餐',exact=True).click()
    await optional(page.get_by_role('button',name='继续进行单程预订',exact=True))
    stage('passenger')
    p=config['passenger']
    await expect(page.get_by_role('heading',name='输入乘客',exact=True)).to_be_visible()
    await page.locator('input#lastName').fill(p['last_name'])
    await page.locator('input#firstName').fill(p['first_name'])
    await page.get_by_text('男' if p['gender']=='M' else '女',exact=True).click()
    await date_fields(page,'出生年月日',p['birthday'])
    await page.get_by_role('button',name='国籍/地区※',exact=True).click()
    await page.get_by_role('option',name=p['nationality_label'],exact=True).click()
    await page.get_by_label('电子邮箱※(输入半角英文字母与数字)',exact=True).fill(p['email'])
    await page.get_by_label('电子邮箱（确认）※',exact=True).fill(p['email'])
    await page.get_by_role('button',name='国家/地区代码',exact=True).first.click()
    await page.get_by_role('option',name=p['phone_code'],exact=True).click()
    await page.get_by_label('电话号码※(输入半角数字)',exact=True).first.fill(p['phone'])
    await page.get_by_label('护照号码※(输入半角英文字母与数字)',exact=True).fill(p['passport'])
    await date_fields(page,'有效期限',p['passport_expiry'])
    await page.get_by_role('link',name='下一步',exact=True).click()
    await optional(page.get_by_role('button',name='确认无误，继续下一步',exact=True))
    await page.get_by_role('button',name='下一步',exact=True).click()
    await page.get_by_text('我已确认购票、搭乘相关的注意事项。',exact=True).click()
    await optional(page.get_by_text('我同意以上所述。',exact=True))
    await page.get_by_role('link',name='下一步',exact=True).click()
    await optional(page.get_by_role('button',name='下一步',exact=True))
    await page.get_by_label('直接输入',exact=True).click()
    await page.get_by_role('option',name=p['last_name']+' '+p['first_name'],exact=True).first.click()
    await page.get_by_role('link',name='下一步',exact=True).click()
    stage('review')
    # A review selector must target the displayed total, never arbitrary body text.
    review=config.get('review',{})
    await expect(page.locator(review.get('flight_selector','[data-testid="review-flight"]'))).to_have_text(config['flight_number'])
    await expect(page.locator(review.get('passenger_selector','[data-testid="review-passenger"]'))).to_have_text(p['last_name']+' '+p['first_name'])
    amount=await page.locator(review.get('amount_selector','[data-testid="review-total"]')).text_content()
    numbers=re.findall(r'\d[\d,]*(?:\.\d+)?',amount)
    if len(numbers)!=1 or Decimal(numbers[0].replace(',',''))!=Decimal(str(config['amount'])) or config['currency'] not in amount:
        raise BookingError('PRICE_MISMATCH')
    await expect(page.get_by_text('其他信用卡',exact=True)).to_be_visible()
    if not offline:
        stage('ready_for_payment')
        return {'status':'READY_FOR_PAYMENT','flight_number':config['flight_number'],'currency':config['currency'],'amount':str(config['amount'])}
    stage('simulated_payment')
    async with page.expect_response('**/v2/mypage/flights/member') as pending:
        await page.get_by_role('button',name='模拟支付',exact=True).click()
    response=await pending.value
    result=await response.json()
    if not response.ok or not result.get('pnr'): raise BookingError('PAYMENT_FAILED')
    pnr=result['pnr']
    if (pnr.get('flightNumber')!=config['flight_number'] or pnr.get('passenger')!=p['last_name']+' '+p['first_name']
        or Decimal(str(pnr.get('amount')))!=Decimal(str(config['amount'])) or pnr.get('currency')!=config['currency']):
        raise BookingError('BOOKING_RESULT_MISMATCH')
    return {'status':'SIMULATED_BOOKED','pnr':pnr['confirmationNumber']}
