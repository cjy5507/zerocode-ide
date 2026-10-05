export async function solveAsPerson(page, card) {
  const fact = (says) => {
    const f = card.facts.find((x) => x.says === says);
    if (!f) throw new Error('card has no fact for: ' + says);
    return f.value;
  };

  // The cookie bar covers the bottom of the window; close it first.
  await page.click('#ck-all');

  // Step 1: account details
  await page.fill('#first', fact('First name'));
  await page.fill('#last', fact('Last name'));
  await page.fill('#email', fact('Email address'));
  await page.fill('#pw', fact('Password'));
  await page.fill('#pw2', fact('Confirm password'));
  const [y, m, d] = String(fact('Date of birth')).split('-');
  await page.click('#dob');
  await page.keyboard.type(`${m}/${d}/${y}`);
  await page.click('#s1 [data-next]');

  // Step 2: mobile number and the code sent to the phone
  await page.waitForSelector('#cc-btn', { state: 'visible' });
  await page.click('#cc-btn');
  await page.locator('#cc-list [role=option]').filter({ hasText: fact('Country code') }).click();
  await page.fill('#mobile', fact('Mobile number'));
  await page.waitForSelector('#send-code:not([disabled])');
  await page.click('#send-code');
  await page.waitForSelector('#otp-sent', { state: 'visible' });
  const code = await page.evaluate(() => window.__personPhone);
  await page.locator('#otp input').first().click();
  await page.keyboard.type(String(code));
  await page.waitForSelector('#verify:not([disabled])');
  await page.click('#verify');
  await page.waitForSelector('#otp-ok', { state: 'visible' });
  await page.click('#next2');

  // Step 3: shipping address
  await page.waitForSelector('#country', { state: 'visible' });
  await page.selectOption('#country', { label: fact('Country / Region') });
  await page.waitForSelector('#region-btn:not([disabled])');
  await page.click('#region-btn');
  await page.locator('#region-list').getByRole('option', { name: fact('Province / Metropolitan city'), exact: true }).click();
  const city = String(fact('City / District'));
  await page.click('#city');
  await page.keyboard.type(city.slice(0, 4));
  await page.locator('#city-list').getByRole('option', { name: city, exact: true }).click();
  await page.fill('#postal', fact('Postal code'));
  await page.fill('#addr1', fact('Street address'));
  const aptSays = 'Apartment, suite, unit, etc. (optional)';
  await page.getByPlaceholder(aptSays, { exact: true }).fill(fact(aptSays));
  await page.locator('#addr-type [role=radio]').filter({ hasText: fact('Address type') }).click();
  await page.fill('#note', fact('Delivery instructions'));
  await page.click('#s3 [data-next]');

  // Step 4: preferences and consents
  await page.waitForSelector('#currency', { state: 'visible' });
  await page.selectOption('#currency', { label: fact('Show prices in') });
  const picks = String(fact('What do you like to shop for?')).split(',').map((s) => s.trim()).filter(Boolean);
  for (const label of picks) {
    await page.locator('#interests .chip').filter({ hasText: label }).click();
  }
  if (fact('Email me about new arrivals, restocks and members-only offers') === true) {
    await page.click('#sw-news');
    await page.locator('#freq label').filter({ hasText: fact('How often should we email you?') }).click();
  }
  const consents = [
    'I agree to the Terms of Service',
    'I agree to the collection and use of my personal information',
    'I agree to my personal information being transferred overseas for delivery',
    'I am 16 years of age or older'
  ];
  for (const says of consents) {
    if (fact(says) === true) {
      await page.locator('#consents label').filter({ hasText: says }).click();
    }
  }
  await page.click('#create');
  await page.waitForSelector('#s5', { state: 'visible' });
}
