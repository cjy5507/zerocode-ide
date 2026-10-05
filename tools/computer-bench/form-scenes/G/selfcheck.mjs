// 사람이 마우스와 키보드로 장바구니부터 주문 완료까지 끝낼 수 있다는 증명.
export async function solveAsPerson(page, card) {
  const fact = (says) => {
    const f = card.facts.find((x) => x.says === says);
    if (!f) throw new Error('card has no fact for ' + says);
    return f.value;
  };

  // 1. 장바구니: 줄마다 − / + 를 눌러 수량을 맞춘다
  for (const name of ['유기농 방울토마토 500g', '무농약 시금치 1단', '햇감귤 3kg']) {
    const want = Number(fact(name));
    const row = page.locator('#cartList .cart-row', { hasText: name });
    const box = row.locator('.qty-input');
    for (let i = 0; i < 20; i += 1) {
      const now = Number(await box.inputValue());
      if (now === want) break;
      await row.locator(now < want ? '.qty-plus' : '.qty-minus').click();
    }
    if (Number(await box.inputValue()) !== want) throw new Error('quantity not reached for ' + name);
  }
  await page.click('#toOrder');
  await page.waitForSelector('#step2', { state: 'visible' });

  // 2. 배송지: 새로운 배송지 → 받는 분, 연락처, 주소 검색 창에서 고르기, 상세 주소
  await page.locator('#shipToGroup label', { hasText: fact('배송지 선택') }).click();
  await page.fill('#rcvName', fact('받는 분'));
  await page.fill('#rcvPhone', fact('연락처'));
  await page.click('#zipBtn');
  const zipFrame = page.frameLocator('#zipFrame');
  await zipFrame.locator('#q').fill(fact('주소'));
  await page.keyboard.press('Enter');
  const results = zipFrame.locator('.res-item');
  await results.first().waitFor();
  const count = await results.count();
  let picked = false;
  for (let i = 0; i < count && !picked; i += 1) {
    const road = ((await results.nth(i).locator('.road-addr').textContent()) || '').trim();
    if (road === fact('주소')) {
      await results.nth(i).click();
      picked = true;
    }
  }
  if (!picked) throw new Error('address not in the search results');
  await page.waitForSelector('#zipLayer', { state: 'hidden' });
  await page.fill('#addr2', fact('상세 주소'));

  // 3. 배송 일정: 달력에서 날짜(필요하면 다음 달로), 불러온 시간대, 요청사항
  const date = fact('배송 희망일');
  await page.click('#dDate');
  await page.waitForSelector('#cal', { state: 'visible' });
  for (let i = 0; i < 6; i += 1) {
    if ((await page.locator(`#cal button[data-date="${date}"]`).count()) > 0) break;
    await page.click('#cal .cal-next');
  }
  await page.click(`#cal button[data-date="${date}"]`);
  await page.waitForSelector('#slotList .slot');
  await page.locator('#slotList .slot', { hasText: fact('배송 시간대') }).click();
  const note = fact('배송 요청사항');
  const chip = page.locator('#noteChips .chip', { hasText: note });
  if ((await chip.count()) === 1 && ((await chip.textContent()) || '').trim() === note) await chip.click();
  else await page.fill('#note', note);

  // 4. 쿠폰
  await page.fill('#couponCode', fact('쿠폰 코드'));
  await page.click('#couponApply');
  await page.waitForSelector('#couponOk', { state: 'visible' });

  // 5. 결제: 결제 수단 → 입금 계좌, 입금자명, 현금영수증
  await page.locator('.pay-card', { hasText: fact('결제 수단') }).click();
  await page.click('#bankBtn');
  await page.locator('#bankList [role="option"]', { hasText: fact('입금 은행') }).click();
  await page.fill('#depositor', fact('입금자명'));
  await page.locator('#receiptGroup label', { hasText: fact('현금영수증') }).click();
  await page.fill('#receiptNo', fact('발급 번호'));

  // 6. 약관: 카드에 적힌 동의만 체크하고 주문
  for (const f of card.facts) {
    if (typeof f.value !== 'boolean') continue;
    const box = page.locator('.agree-item', { hasText: f.says }).locator('input[type="checkbox"]');
    if (f.value) await box.check();
    else await box.uncheck();
  }
  await page.click('#submitOrder');
  await page.waitForSelector('#step3', { state: 'visible' });
}
