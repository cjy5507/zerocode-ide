# Order pricing

`price_order(lines, member=False, coupon=None, date=..., weight_g=0)` prices one order. All money
is in whole cents. `lines` is a list of `(unit_cents, quantity)`. `date` is an ISO date such as
`2026-03-01`. `coupon` is `{"kind": "percent" | "fixed", "value": ..., "expires": ...,
"min_subtotal": ...}`, with `min_subtotal` optional. It answers a dict with `gross`,
`line_discount`, `coupon_discount`, `subtotal`, `shipping`, `tax` and `total`.

- **R1 Input.** An order has at least one line. A quantity is a whole number of at least 1 and a
  unit price is at least 0. Anything else raises `ValueError`. `gross` is the sum of unit price
  times quantity.
- **R2 Volume tiers.** A line's quantity sets its rate: 5% off from 10 units, 10% off from 50
  units and 15% off from 100 units. Each figure is the first quantity that gets that rate.
- **R3 Member price.** A member gets a further 3% off what is left of each line after its volume
  tier. The two percentages multiply; they are not added together.
- **R4 Line rounding.** A line's price after R2 and R3 is rounded half up to a whole cent, once,
  at the end. `line_discount` is `gross` minus the sum of the lines' prices.
- **R5 Coupon eligibility.** A coupon applies when the sum of the lines' prices is at least
  `min_subtotal` and `date` is not after `expires`: it still works on the day it expires. A coupon
  that does not apply is ignored, and is not an error.
- **R6 Coupon amount.** A percent coupon takes `value` percent of the sum of the lines' prices,
  rounded half up to a whole cent. A fixed coupon takes `value` cents, but never more than that
  sum.
- **R7 Discount cap.** `line_discount` plus `coupon_discount` may not exceed 40% of `gross`, rounded
  down to a whole cent. When it would, `coupon_discount` is reduced until it fits; line discounts
  are never reduced.
- **R8 Shipping.** `subtotal` is the sum of the lines' prices minus `coupon_discount`. Shipping is
  499 when `subtotal` is below 5000, and 0 from 5000 on.
- **R9 Heavy parcels.** Each kilogram or part of a kilogram above 20 kg adds 50 to `shipping`,
  whether or not the rest of shipping is free.
- **R10 Tax.** Tax is 8.25% of `subtotal`. Shipping is not taxed. It is rounded half to even.
- **R11 Total.** `total` is `subtotal` plus `shipping` plus `tax`.
