# How an invoice must add up

These rules hold for every invoice.

1. Every amount is a whole number of the currency's smallest unit. USD has 2 decimal places,
   JPY has 0 and KWD has 3. An amount never passes through a float. A price written with more
   decimal places than its currency has, or a currency that is not one of these, is a
   `ValueError`.
2. A line's gross is its unit price times its quantity. A negative quantity makes a credit
   line. One invoice holds only ordinary lines or only credit lines; a mix is a `ValueError`.
3. The order discount is `discount_percent` of the subtotal (the sum of gross). It may hold
   decimals, like "12.5". It is rounded half up in magnitude to a whole smallest unit, so
   17.5 becomes 18 and -17.5 becomes -18.
4. The discount is shared among the lines in proportion to their gross. Each line first takes
   the whole part of its exact share. The units left over go, one each, to the lines with the
   largest fractional part; an earlier line wins a tie. The shares add up to the discount
   exactly.
5. A line's net is its gross minus its share.
6. Tax is worked out once per tax class, on the sum of the net of that class's lines, and
   rounded half to even: 241.5 becomes 242 and 240.5 becomes 240. It is not worked out per line.
7. The invoice total is the sum of net plus the sum of tax.
8. Posting an invoice to the ledger debits `receivable` with the total, credits `revenue`
   with the sum of net, and credits `tax:<class>` with each class's tax. A debit is positive
   and a credit is negative. An entry whose amounts do not add up to exactly zero is refused
   with `UnbalancedEntry`, and nothing of it is recorded. No difference is ever written off to
   another account. `balance` answers an integer in the smallest unit, kept apart per currency.
9. The export prints each amount with exactly its currency's decimal places and a leading `-`
   when negative: `1234.50`, `1234`, `12.345`, `-0.35`. Its layout does not change.
