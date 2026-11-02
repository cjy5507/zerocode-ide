"""Order pricing: see SPEC.md."""

PERCENT = 100
BASIS = 10_000
MEMBER_OFF = 300
CAP_PERCENT = 40
FREE_SHIPPING_FROM = 5000
FLAT_SHIPPING = 499
FREE_WEIGHT_G = 20_000
SURCHARGE_PER_KG = 50
TAX_BASIS = 825


def _half_up(numerator, denominator):
    return (2 * numerator + denominator) // (2 * denominator)


def _tier_off(quantity):
    if quantity >= 100:
        return 1500
    if quantity > 50:
        return 1000
    if quantity >= 10:
        return 500
    return 0


def _line_price(unit, quantity, member):
    off = _tier_off(quantity) + (MEMBER_OFF if member else 0)
    return _half_up(unit * quantity * (BASIS - off), BASIS)


def _coupon_amount(coupon, sum_of_lines, date):
    if coupon is None:
        return 0
    if date > coupon["expires"] or sum_of_lines < coupon.get("min_subtotal", 0):
        return 0
    if coupon["kind"] == "percent":
        return _half_up(sum_of_lines * coupon["value"], PERCENT)
    return coupon["value"] if coupon["value"] < sum_of_lines else sum_of_lines


def _shipping(subtotal, weight_g):
    base = 0 if subtotal >= FREE_SHIPPING_FROM else FLAT_SHIPPING
    above = max(weight_g - FREE_WEIGHT_G, 0)
    return base + above // 1000 * SURCHARGE_PER_KG


def _tax(subtotal):
    return _half_up(subtotal * TAX_BASIS, BASIS)


def price_order(lines, member=False, coupon=None, date="2026-01-01", weight_g=0):
    """The price of an order: see SPEC.md."""
    if not lines:
        raise ValueError("an order has at least one line")
    for unit, quantity in lines:
        if not isinstance(quantity, int) or quantity < 1 or unit < 0:
            raise ValueError(f"not a line: {unit} x {quantity}")
    gross = sum(unit * quantity for unit, quantity in lines)
    sum_of_lines = sum(_line_price(unit, quantity, member) for unit, quantity in lines)
    line_discount = gross - sum_of_lines
    room = max(sum_of_lines * CAP_PERCENT // PERCENT - line_discount, 0)
    coupon_discount = min(_coupon_amount(coupon, sum_of_lines, date), room)
    subtotal = sum_of_lines - coupon_discount
    shipping = _shipping(subtotal, weight_g)
    tax = _tax(subtotal)
    return {
        "gross": gross,
        "line_discount": line_discount,
        "coupon_discount": coupon_discount,
        "subtotal": subtotal,
        "shipping": shipping,
        "tax": tax,
        "total": subtotal + shipping + tax,
    }
