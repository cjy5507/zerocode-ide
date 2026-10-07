// Proves scene.html is solvable by a person using only a mouse and keyboard.
// The bench opens scene.html in a Playwright page, calls solveAsPerson(page, card),
// then compares window.__sceneResult with expected.json.

export async function solveAsPerson(page, card) {
  const T = 20000;

  const fact = (says) => {
    const f = card.facts.find((x) => x.says === says);
    if (!f) throw new Error('card has no fact: ' + says);
    return f.value;
  };

  async function pickDate(box, iso) {
    const [y, m, d] = iso.split('-').map(Number);
    await page.click(box + ' input');
    await page.waitForSelector(box + ' .cal-h strong', { timeout: T });
    for (let i = 0; i < 40; i++) {
      const title = ((await page.textContent(box + ' .cal-h strong')) || '').trim();
      const mm = /(\d+)\s*년\s*(\d+)\s*월/.exec(title);
      if (!mm) throw new Error('cannot read calendar title: ' + title);
      const cy = Number(mm[1]);
      const cm = Number(mm[2]);
      if (cy === y && cm === m) break;
      const dir = cy * 12 + cm < y * 12 + m ? 1 : -1;
      await page.click(box + ' .cal-nav[data-nav="' + dir + '"]');
    }
    await page
      .locator(box + ' .cal .day:not([disabled])', { hasText: new RegExp('^' + d + '$') })
      .click();
  }

  async function pickTime(hSel, mSel, hhmm) {
    const [h, mi] = hhmm.split(':');
    await page.selectOption(hSel, { label: h });
    await page.selectOption(mSel, { label: mi });
  }

  // ---------- step 1 ----------
  await page.waitForSelector('#dd-terminal .dd-btn', { timeout: T });
  await page.click('#dd-terminal .dd-btn');
  await page.locator('#dd-terminal li', { hasText: fact('이용 터미널') }).click();

  // the parking-lot list loads a moment after the terminal is chosen
  await page.waitForSelector('#dd-lot:not(.disabled)', { timeout: T });
  await page.click('#dd-lot .dd-btn');
  await page.locator('#dd-lot li', { hasText: fact('주차장') }).click();

  await pickDate('#db-in', fact('입차 날짜'));
  await pickTime('#in-h', '#in-m', fact('입차 시각'));
  await pickDate('#db-out', fact('출차 날짜'));
  await pickTime('#out-h', '#out-m', fact('출차 시각'));

  await page.getByLabel(fact('공항 셔틀 이용'), { exact: true }).check();
  await page.waitForSelector('#pax', { timeout: T });
  await page.selectOption('#pax', { label: fact('탑승 인원') });

  await page.click('#btn-next');

  // ---------- step 2 ----------
  await page.waitForSelector('#v-plate', { state: 'visible', timeout: T });
  await page.fill('#v-plate', fact('차량번호'));
  await page.selectOption('#v-type', { label: fact('차종') });
  await page.fill('#v-color', fact('차량 색상'));
  await page.fill('#d-name', fact('운전자 성명'));

  const [by, bm, bd] = fact('생년월일').split('-').map(Number);
  await page.selectOption('#b-y', { label: String(by) });
  await page.selectOption('#b-m', { label: String(bm) });
  await page.selectOption('#b-d', { label: String(bd) });

  const [p1, p2, p3] = fact('휴대전화').split('-');
  await page.selectOption('#p-1', { label: p1 });
  await page.fill('#p-2', p2);
  await page.fill('#p-3', p3);
  await page.click('#btn-otp');
  await page.waitForSelector('#otp-in', { state: 'visible', timeout: T });
  const code = await page.evaluate(() => window.__personPhone);
  if (!code) throw new Error('no code was sent to the phone');
  await page.fill('#otp-in', String(code));
  await page.click('#otp-ok');
  await page.waitForSelector('text=휴대전화 인증이 완료되었습니다', { timeout: T });

  const [local, domain] = String(fact('이메일')).split('@');
  await page.fill('#e-id', local);
  const domainOptions = await page.$$eval('#e-sel option', (os) => os.map((o) => o.textContent.trim()));
  if (domainOptions.includes(domain)) {
    await page.selectOption('#e-sel', { label: domain });
  } else {
    await page.selectOption('#e-sel', { label: '직접 입력' });
    await page.fill('#e-dom', domain);
  }

  await page.fill('#memo', fact('요청 사항 (선택)'));
  await page.click('#btn-next');

  // ---------- step 3 ----------
  const fr = page.frameLocator('#pay-frame');
  await fr.locator('#c1').waitFor({ state: 'visible', timeout: T });
  const digits = String(fact('카드 번호')).replace(/\D/g, '');
  await fr.locator('#c1').fill(digits.slice(0, 4));
  await fr.locator('#c2').fill(digits.slice(4, 8));
  await fr.locator('#c3').fill(digits.slice(8, 12));
  await fr.locator('#c4').fill(digits.slice(12, 16));
  const [xm, xy] = String(fact('유효기간')).split('/');
  await fr.locator('#xm').fill(xm);
  await fr.locator('#xy').fill(xy);
  await fr.locator('#cn').fill(String(fact('카드 소유자명')));
  await fr.locator('#cv').fill(String(fact('CVC')));
  await fr.locator('#inst').selectOption({ label: fact('할부') });

  await page.waitForSelector('#ag-terms', { state: 'visible', timeout: T });
  if (fact('이용약관 동의 (필수)')) await page.click('#ag-terms');
  if (fact('개인정보 수집·이용 동의 (필수)')) await page.click('label.chk2');
  if (fact('마케팅 정보 수신 동의 (선택)')) await page.click('#ag-mkt');

  await page.click('#btn-next');
  await page.waitForSelector('#done:not([hidden])', { timeout: T });
  await page.waitForFunction(() => !!window.__sceneResult, null, { timeout: T });
}
