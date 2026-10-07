export async function solveAsPerson(page, card) {
  const facts = card.facts || [];
  const has = (says) => facts.some((f) => f.says === says);
  const fact = (says) => {
    const f = facts.find((x) => x.says === says);
    if (!f) throw new Error('card has no fact for: ' + says);
    return f.value;
  };
  const exact = (text) => new RegExp('^\\s*' + String(text).replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '\\s*$');

  // 1. 진료 선택
  await page.locator('#treatGroup label.treat-card', { hasText: String(fact('진료 항목')) }).click();
  await page.locator('#visitType label', { hasText: String(fact('방문 구분')) }).click();
  if (has('어떤 증상이 있으신가요?')) {
    await page.waitForSelector('#symptomBlock .chip');
    const wanted = String(fact('어떤 증상이 있으신가요?')).split(',').map((s) => s.trim()).filter(Boolean);
    for (const name of wanted) {
      await page.locator('#symptomChips .chip', { hasText: exact(name) }).click();
    }
  }
  if (has('현재 통증 정도')) {
    const level = Number(fact('현재 통증 정도'));
    await page.locator('#pain').click();
    await page.keyboard.press('Home');
    for (let i = 0; i < level; i++) await page.keyboard.press('ArrowRight');
  }
  await page.click('#btnNext');

  // 2. 지점·의료진
  await page.waitForSelector('#step2:not([hidden])');
  await page.click('#branchBtn');
  await page.locator('#branchList [role="option"]', { hasText: String(fact('지점')) }).click();
  await page.locator('#doctorList .doc-card', { hasText: String(fact('담당 의료진')) }).click();
  await page.click('#btnNext');

  // 3. 날짜·시간
  await page.waitForSelector('#step3:not([hidden])');
  const date = String(fact('예약 날짜'));
  const [year, month] = date.split('-').map(Number);
  await page.click('#dateInput');
  await page.waitForSelector('#calPop:not([hidden])');
  for (let i = 0; i < 24; i++) {
    const title = ((await page.locator('#calTitle').textContent()) || '').trim();
    if (title === `${year}년 ${month}월`) break;
    await page.click('#calNext');
  }
  await page.click(`#calBody button.day[data-date="${date}"]`);
  await page.locator('#timeArea button.slot', { hasText: exact(fact('예약 시간')) }).click();
  await page.click('#btnNext');

  // 4. 예약자 정보
  await page.waitForSelector('#step4:not([hidden])');
  await page.fill('#name', String(fact('이름')));
  await page.fill('#birth', String(fact('생년월일')));
  await page.fill('#phone', String(fact('휴대폰 번호')));
  await page.click('#btnSendCode');
  await page.waitForSelector('#codeRow:not([hidden])');
  const code = await page.evaluate(() => window.__personPhone);
  await page.click('#smsCode');
  await page.keyboard.type(String(code));
  await page.click('#btnVerify');
  await page.waitForSelector('#phoneField.is-verified');
  if (has('이메일')) await page.fill('#email', String(fact('이메일')));
  if (fact('개인정보 수집·이용 동의 (필수)') === true) await page.locator('#agreePrivacy').check();
  if (fact('민감정보(건강정보) 처리 동의 (필수)') === true) await page.locator('#agreeHealth').check();
  const wantNews = has('진료 안내·이벤트 소식 받기 (선택)') && fact('진료 안내·이벤트 소식 받기 (선택)') === true;
  const newsOn = (await page.locator('#mkSwitch').getAttribute('aria-checked')) === 'true';
  if (newsOn !== wantNews) await page.click('#mkSwitch');
  await page.click('#btnNext');

  // 5. 확인 후 신청
  await page.waitForSelector('#step5:not([hidden])');
  await page.click('#btnSubmit');
  await page.waitForSelector('#doneScreen:not([hidden])');
}
