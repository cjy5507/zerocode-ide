// Completes scene.html the way a person would: clicking visible things by text/role,
// typing on the keyboard, scrolling, and reading the code from the "phone".
export async function solveAsPerson(page, card) {
  const fact = (says) => {
    const f = card.facts.find((x) => x.says === says);
    if (!f) throw new Error('card has no fact for: ' + says);
    return f.value;
  };
  const hasFact = (says) => card.facts.some((x) => x.says === says);
  const parseDate = (s) => {
    const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(String(s));
    if (!m) throw new Error('not a date: ' + s);
    return { y: Number(m[1]), mo: Number(m[2]), d: Number(m[3]) };
  };

  // ---- 대여 기간: open the range calendar, page to the month, click start then end, apply ----
  await page.getByRole('button', { name: /^출발/ }).click();
  const dlg = page.getByRole('dialog', { name: '대여 기간 선택' });
  await dlg.waitFor();
  const clickDay = async (iso) => {
    const { y, mo, d } = parseDate(iso);
    const month = dlg.locator('.month').filter({
      has: page.getByRole('heading', { name: `${y}년 ${mo}월`, exact: true }),
    });
    for (let i = 0; i < 14 && (await month.count()) === 0; i++) {
      await dlg.getByRole('button', { name: '다음 달' }).click();
    }
    await month.getByRole('button', { name: `${mo}월 ${d}일`, exact: true }).click();
  };
  await clickDay(fact('대여 기간'));
  await clickDay(fact('대여 기간 반납'));
  await dlg.getByRole('button', { name: '선택 완료' }).click();
  await dlg.waitFor({ state: 'hidden' });

  // ---- 인수 시간: scroll the list and click the time ----
  const pickup = fact('인수 시간');
  await page.getByRole('option', { name: pickup, exact: true }).click();

  // ---- 반납 시간: 오전/오후 + 시 + 분 ----
  const rt = /^(오전|오후)\s*(\d{1,2}):(\d{2})$/.exec(fact('반납 시간'));
  if (!rt) throw new Error('return time not understood: ' + fact('반납 시간'));
  await page.getByRole('radio', { name: rt[1], exact: true }).click();
  await page.getByLabel('시', { exact: true }).selectOption({ label: `${Number(rt[2])}시` });
  await page.getByLabel('분', { exact: true }).selectOption({ label: `${rt[3]}분` });

  // ---- 차량 등급, then the model list that loads a moment later ----
  await page.getByRole('radio', { name: fact('차량 등급') }).check();
  await page.getByLabel('모델').selectOption({ label: fact('모델') });

  // ---- 성인 / 어린이 steppers (start at 1 and 0) ----
  const stepTo = async (name, from, to) => {
    for (let i = from; i < to; i++) await page.getByRole('button', { name: `${name} 늘리기` }).click();
    for (let i = from; i > to; i--) await page.getByRole('button', { name: `${name} 줄이기` }).click();
  };
  await stepTo('성인', 1, Number(fact('성인')));
  await stepTo('어린이', 0, Number(fact('어린이')));

  // ---- 추가 옵션 chips: make each one match what the person wants ----
  for (const words of ['자전거 거치대', '캠핑 의자 세트', '휴대용 버너', '침구 세트', '차박 모기장']) {
    if (!hasFact(words)) continue;
    const want = fact(words) === true;
    const chip = page.getByRole('button', { name: new RegExp(words) });
    const on = (await chip.getAttribute('aria-pressed')) === 'true';
    if (on !== want) await chip.click();
  }

  // ---- 보장 범위, 인수 방법 ----
  await page.getByRole('radio', { name: fact('보장 범위') }).check();
  await page.getByRole('radio', { name: fact('인수 방법') }).check();

  // ---- 배송 주소: type, wait for suggestions, choose the exact one ----
  const addrWords = fact('배송 주소');
  const addr = page.getByLabel('배송 주소');
  await addr.click();
  await page.keyboard.type(addrWords, { delay: 30 });
  await page
    .getByRole('option')
    .filter({ has: page.getByText(addrWords, { exact: true }) })
    .click();
  await page.getByLabel('상세 주소').fill(fact('상세 주소'));

  // ---- 운전자 ----
  await page.getByLabel('운전자 성명').fill(fact('운전자 성명'));
  const b = parseDate(fact('생년월일'));
  await page.getByLabel('년', { exact: true }).selectOption({ label: `${b.y}년` });
  await page.getByLabel('월', { exact: true }).selectOption({ label: `${b.mo}월` });
  await page.getByLabel('일', { exact: true }).selectOption({ label: `${b.d}일` });
  await page.getByLabel('면허 종류').selectOption({ label: fact('면허 종류') });
  await page.getByLabel('면허번호').fill(fact('면허번호'));
  await page.getByLabel('이메일').fill(fact('이메일'));

  // ---- 휴대전화 + 문자로 온 인증번호 ----
  await page.getByLabel('휴대전화').fill(fact('휴대전화'));
  await page.getByRole('button', { name: '인증번호 받기' }).click();
  await page.waitForFunction(
    () => typeof window.__personPhone === 'string' && window.__personPhone.length > 0,
  );
  const code = await page.evaluate(() => window.__personPhone);
  await page.getByLabel('인증번호').fill(code);
  await page.getByRole('button', { name: '인증 확인' }).click();
  await page.getByText('인증이 완료되었습니다').waitFor();

  // ---- 요청 사항 ----
  await page.getByLabel('요청 사항').fill(fact('요청 사항'));

  // ---- 약관: scroll the text to its end, then the box can be ticked ----
  const termsSays = '대여 약관에 동의합니다 (필수)';
  const agree = page.getByRole('checkbox', { name: termsSays });
  const terms = page.getByRole('region', { name: /대여 약관/ });
  await terms.scrollIntoViewIfNeeded();
  await terms.hover();
  for (let i = 0; i < 40 && !(await agree.isEnabled()); i++) {
    await page.mouse.wheel(0, 400);
  }
  if (fact(termsSays) === true) await agree.check();

  const mktSays = '이벤트·소식 문자 수신 (선택)';
  const mkt = page.getByRole('checkbox', { name: mktSays });
  if (fact(mktSays) === true) await mkt.check();
  else if (await mkt.isChecked()) await mkt.uncheck();

  // ---- 신청 ----
  await page.getByRole('button', { name: '예약 신청하기' }).click();
  await page.getByRole('heading', { name: '예약이 접수되었습니다' }).waitFor();
}
