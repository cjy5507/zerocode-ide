// Registers the attendees the way a person does: clicks, typing and choosing from lists.
export async function solveAsPerson(page, card) {
  const fact = (says) => {
    const f = card.facts.find((x) => x.says === says);
    if (!f) throw new Error('card has no fact for: ' + says);
    return f.value;
  };
  const exact = (text) =>
    new RegExp('^\\s*' + String(text).replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '\\s*$');

  // 1. Tickets: the pass card, then the stepper up to the number of attendees.
  await page.locator('.tickets .t-name', { hasText: exact(fact('Choose your pass')) }).click();
  const count = parseInt(fact('Number of attendees'), 10);
  for (let n = 1; n < count; n++) await page.click('#qtyPlus');
  await page.click('#next1');

  // 2. Attendees: one block each. The session list opens once the sessions for the pass are in.
  await page.waitForSelector('#attendeeList .att');
  for (let i = 1; i <= count; i++) {
    const p = 'Attendee ' + i + ': ';
    const block = page.locator('#attendeeList .att').nth(i - 1);
    await block.locator('input[data-f="first"]').fill(String(fact(p + 'First name')));
    await block.locator('input[data-f="last"]').fill(String(fact(p + 'Last name')));
    await block.locator('input[data-f="email"]').fill(String(fact(p + 'Work e-mail')));
    await block.locator('input[data-f="title"]').fill(String(fact(p + 'Job title')));
    await block.locator('.chip', { hasText: exact(fact(p + 'Dietary preference')) }).click();
    await block.locator('.dd-btn').click();
    await block.locator('.dd-list .o-t', { hasText: exact(fact(p + 'Session track')) }).click();
  }
  await page.click('#next2');

  // 3. Contact & billing.
  const cc = String(fact('Country code'));
  await page.click('#ccBtn');
  await page.keyboard.type(cc.replace(/\s*\+\d+$/, ''));
  await page.locator('#ccList li', { hasText: exact(cc) }).click();
  await page.fill('#phone', String(fact('Mobile number')));

  if (fact('I need an invoice') === true) {
    await page.click('#invoiceSw');
    await page.fill('#company', String(fact('Company name')));
    await page.selectOption('#bCountry', { label: String(fact('Billing country')) });
    await page.fill('#taxNo', String(fact('Tax number')));
    await page.fill('#line1', String(fact('Address line 1')));
    await page.fill('#city', String(fact('City')));
    await page.fill('#postcode', String(fact('Postal code')));
  }

  if (fact('I have read and agree to the Registration Terms and the Code of Conduct.') === true) {
    await page.check('#terms');
  }
  if (fact('I confirm that every attendee listed has agreed to their details being shared with Quillfern Events for this registration.') === true) {
    await page.check('#dataConsent');
  }
  await page.click('#next3');

  // 4. Review, then complete and wait for the confirmation.
  await page.click('#submitBtn');
  await page.waitForSelector('#doneBox', { state: 'visible' });
}
