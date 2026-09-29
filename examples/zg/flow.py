"""ZG single-adult one-way checkout using official Playwright Python."""
from datetime import datetime
from decimal import Decimal
import re
from playwright.async_api import TimeoutError, expect


class BookingError(Exception):
    pass


def validate(config):
    for key in ('origin','destination','departure_date','currency','passenger'):
        if key not in config: raise BookingError('CONFIG_MISSING_'+key.upper())
    datetime.strptime(config['departure_date'],'%Y-%m-%d')
    if config.get('flight_number') and not re.fullmatch(r'ZG\d{3,4}',config['flight_number']): raise BookingError('INVALID_FLIGHT_NUMBER')
    if config.get('amount') is not None and Decimal(str(config['amount']))<=0: raise BookingError('INVALID_AMOUNT')
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
    except TimeoutError: pass


async def date_fields(page, group, date):
    year,month,day=date.split('-')
    root=page.get_by_role('group',name=group)
    await root.get_by_role('button',name='日',exact=True).click()
    await page.get_by_role('option',name=str(int(day)),exact=True).click()
    await root.get_by_role('button',name='月',exact=True).click()
    await page.get_by_role('option',name=datetime.strptime(month,'%m').strftime('%b'),exact=True).click()
    await root.get_by_role('button',name='年',exact=True).click()
    await page.get_by_role('option',name=year,exact=True).click()


async def checkout_live(page, config, stage):
    if (config['origin'] != '吉隆坡 KUL' or config['destination'] != '东京 NRT'
            or config['currency'] != 'USD - US$'):
        raise BookingError('LIVE_SUPPORTS_KUL_NRT_USD_ONLY')
    p=config['passenger']
    page.set_default_timeout(30000)
    stage('landing')
    response=await page.goto(config.get('base_url','https://www.zipair.net/zh-cn/'),wait_until='domcontentloaded')
    for _ in range(2):
        if not response or response.status!=403: break
        response=await page.reload(wait_until='domcontentloaded')
    if response and response.status==403: raise BookingError('SITE_BLOCKED_AT_LANDING')
    await page.wait_for_function('() => window.$nuxt && window.$nuxt._isMounted')
    consent=page.get_by_role('button',name='同意',exact=True)
    for _ in range(4):
        await consent.click()
        try:
            await expect(consent).to_be_hidden(timeout=2000)
            break
        except AssertionError:
            pass
    else:
        raise BookingError('SITE_NOT_HYDRATED')

    stage('search')
    one_way=page.get_by_role('tab',name='单程包括中转')
    for _ in range(4):
        await one_way.click()
        try:
            await expect(one_way).to_have_attribute('aria-selected','true',timeout=2000)
            break
        except AssertionError:
            pass
    else:
        raise BookingError('ONE_WAY_NOT_SELECTED')
    stage('search_origin')
    await page.get_by_role('button',name='出发地（国家，地区）').click()
    await page.get_by_role('button').filter(has_text='吉隆坡KUL').last.click()
    await expect(page.get_by_role('button',name='出发地（国家，地区）')).to_contain_text('KUL')
    stage('search_destination')
    await page.get_by_role('button',name='目的地（国家，地区）').click()
    await page.get_by_role('button').filter(has_text='东京NRT').first.click()
    await expect(page.get_by_role('button',name='目的地（国家，地区）')).to_contain_text('NRT')
    stage('search_submit')
    await page.get_by_text('搜索机票',exact=True).click()
    await expect(page.get_by_role('dialog')).to_be_visible()
    await page.get_by_role('dialog').get_by_role('button',name='下一步').click()
    await expect(page.get_by_role('heading',name='选择人数')).to_be_visible()
    stage('people')
    for _ in range(4):
        await page.get_by_role('button',name='下一步').click()
        try:
            await expect(page.get_by_role('heading',name='选择日期')).to_be_visible(timeout=4000)
            break
        except AssertionError:
            pass
    else:
        raise BookingError('PEOPLE_NEXT_INERT')

    stage('flight')
    await page.locator(f'button[data-fulldate="{config["departure_date"]}"]').click()
    await expect(page.locator('div.cabin-contents').first).to_be_visible(timeout=180000)
    selected=None
    for flight in await page.locator('div.cabin-contents').all():
        start=(await flight.locator('span.start').text_content()).strip()
        if config.get('departure_time') and start!=config['departure_time']: continue
        next_button=page.get_by_role('button',name='下一步',exact=True)
        standard=flight.get_by_role('radio').first
        await flight.get_by_text('座位类型Standard',exact=True).click()
        await expect(standard).to_be_checked()
        await expect(next_button).to_be_enabled()
        selected={'departure_time':start}
        break
    if not selected: raise BookingError('FLIGHT_MISMATCH')
    await page.get_by_role('button',name='下一步',exact=True).click()
    await page.get_by_role('link',name=re.compile('^不要买套餐')).click()
    await optional(page.get_by_role('button',name='继续进行单程预订',exact=True))

    stage('passenger')
    await expect(page.get_by_role('heading',name=re.compile('输入乘客'))).to_be_visible()
    await page.locator('#lastName').fill(p['last_name'])
    await page.locator('#firstName').fill(p['first_name'])
    await page.get_by_text('男' if p['gender']=='M' else '女',exact=True).click()
    await date_fields(page,'出生年月日',p['birthday'])
    await page.get_by_role('button',name='国籍/地区※').click()
    await page.get_by_role('option',name=p['nationality_label'],exact=True).click()
    await page.get_by_label('电子邮箱※(输入半角英文字母与数字)',exact=True).fill(p['email'])
    await page.get_by_label('电子邮箱（确认）※(输入半角英文字母与数字)',exact=True).fill(p['email'])
    await page.get_by_role('button',name='国家/地区代码').click()
    await page.get_by_role('option',name=p['phone_code'],exact=True).click()
    await page.get_by_label('电话号码※(输入半角数字)',exact=True).fill(p['phone'])
    await page.get_by_label('护照号码※(输入半角英文字母与数字)',exact=True).fill(p['passport'])
    await date_fields(page,'有效期限',p['passport_expiry'])
    await page.get_by_role('link',name='下一步',exact=True).click()

    stage('review')
    await expect(page.get_by_role('heading',name='添加可选服务')).to_be_visible()
    await page.get_by_role('button',name='下一步',exact=True).click()
    await expect(page.get_by_role('heading',name='预订确认')).to_be_visible()
    flight_detail=await page.locator('body').inner_text()
    flight_match=re.search(r'ZG\s*0*(\d{2,4})\s*[・·]Standard',flight_detail)
    if not flight_match: raise BookingError('FLIGHT_NUMBER_MISSING_AT_REVIEW')
    selected['flight_number']='ZG'+flight_match.group(1).zfill(3)
    if config.get('flight_number') and selected['flight_number']!=config['flight_number']:
        raise BookingError('FLIGHT_MISMATCH')
    date=datetime.strptime(config['departure_date'],'%Y-%m-%d')
    if f'{date.year}年{date.month}月{date.day}日' not in flight_detail:
        raise BookingError('DATE_MISMATCH')
    await expect(page.get_by_text(p['last_name']+' '+p['first_name'],exact=False).first).to_be_visible()
    await page.get_by_role('checkbox',name='我已确认购票、搭乘相关的注意事项。').check()
    await page.get_by_role('link',name='下一步',exact=True).click()
    await page.get_by_role('button',name='下一步',exact=True).click()
    await expect(page.get_by_role('heading',name='指定收据的收件人')).to_be_visible()
    await page.get_by_label('姓※(输入半角英文字母与数字)',exact=True).fill(p['last_name'])
    await page.get_by_label('名※(输入半角英文字母与数字)',exact=True).fill(p['first_name'])
    await page.get_by_label('电子邮箱※(输入半角英文字母与数字)',exact=True).fill(p['email'])
    await page.get_by_label('电子邮箱（确认）※(输入半角英文字母与数字)',exact=True).fill(p['email'])
    await page.get_by_role('link',name='下一步',exact=True).click()
    await expect(page.get_by_role('heading',name='选择支付信息')).to_be_visible()
    amount=await page.locator('body').inner_text()
    expected=re.search(r'US\$\s*([\d,]+(?:\.\d+)?)',amount)
    if not expected: raise BookingError('PRICE_MISMATCH')
    if config.get('amount') is not None and Decimal(expected.group(1).replace(',',''))!=Decimal(str(config['amount'])):
        raise BookingError('PRICE_MISMATCH')
    stage('ready_for_payment')
    return {'status':'READY_FOR_PAYMENT',**selected,'currency':'USD','amount':expected.group(1).replace(',','')}


async def checkout(page, config, credentials, *, fixture=False, stage=lambda name:None):
    validate(config)
    if not fixture:
        return await checkout_live(page,config,stage)
    if not config['base_url'].startswith('http://127.0.0.1:'):
        raise BookingError('OFFLINE_REQUIRES_LOOPBACK')
    page.set_default_timeout(5000)
    stage('login')
    await page.goto(config['base_url'])
    stage('cookie_consent')
    cookie_consent=page.get_by_role('button',name='同意',exact=True)
    await cookie_consent.click()
    await cookie_consent.wait_for(state='hidden')
    stage('login_form')
    if not await page.get_by_role('link',name='登录',exact=True).is_visible():
        await page.get_by_role('button',name='打开菜单',exact=True).click()
    await page.get_by_role('link',name='登录',exact=True).click()
    await page.get_by_label('电子邮箱※',exact=True).fill(credentials['account'])
    await page.get_by_label('密码※',exact=True).fill(credentials['password'])
    await page.get_by_role('button',name='登录',exact=True).click()
    stage('currency')
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
    selected=None
    for flight in await page.locator('div.cabin-contents').all():
        parts=[await item.text_content() for item in await flight.locator(config.get('flight_number_selector','span')).all()]
        start=(await flight.locator('span.start').text_content()).strip()
        numbers=[match.group(1) for text in parts if (match:=re.fullmatch(r'\s*ZG\s*(\d{1,4})\s*',text))]
        if not numbers or config.get('flight_number') and not any(int(n)==int(config['flight_number'][2:]) for n in numbers):
            continue
        if config.get('departure_time') and start!=config['departure_time']:
            continue
        await optional(flight.get_by_text('选择座位类型',exact=True))
        await flight.get_by_text('座位类型Standard',exact=True).click()
        selected={'flight_number':'ZG'+numbers[0].zfill(3),'departure_time':start}
        break
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
    review=config.get('review',{})
    await page.get_by_text('我已确认购票、搭乘相关的注意事项。',exact=True).click()
    await optional(page.get_by_text('我同意以上所述。',exact=True))
    await page.get_by_role('link',name='下一步',exact=True).click()
    await optional(page.get_by_role('button',name='下一步',exact=True))
    await page.get_by_label('直接输入',exact=True).click()
    await page.get_by_role('option',name=p['last_name']+' '+p['first_name'],exact=True).first.click()
    await page.get_by_role('link',name='下一步',exact=True).click()
    stage('review')
    flight_selector=review.get('flight_selector','[data-testid="review-flight"]')
    passenger_selector=review.get('passenger_selector','[data-testid="review-passenger"]')
    amount_selector=review.get('amount_selector','[data-testid="review-total"]')
    await expect(page.locator(flight_selector)).to_have_text(selected['flight_number'])
    await expect(page.locator(passenger_selector)).to_have_text(p['last_name']+' '+p['first_name'])
    amount=await page.locator(amount_selector).first.text_content()
    numbers=re.findall(r'\d[\d,]*(?:\.\d+)?',amount)
    currency=config['currency'].split(' - ')[0]
    if (len(numbers)!=1 or Decimal(numbers[0].replace(',',''))<=0 or
        config.get('amount') is not None and Decimal(numbers[0].replace(',',''))!=Decimal(str(config['amount'])) or
        not any(label in amount for label in (currency, config['currency'].split(' - ')[-1]))):
        raise BookingError('PRICE_MISMATCH')
    await expect(page.get_by_text('其他信用卡',exact=True)).to_be_visible()
    stage('ready_for_payment')
    return {'status':'READY_FOR_PAYMENT',**selected,
            'currency':config['currency'],'amount':numbers[0].replace(',','')}
