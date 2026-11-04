//! t-93450 r5: a screen where money may leave is the person's (the person's
//! call, 2026-11-04). There only a vertical scroll and a press that backs out
//! are the agent's; every other press, drag and `enter` is refused. Every
//! screen here is in the shape WebDriverAgent answers (`XCUIElementType…`,
//! `label`, `value`, `rect`), read through [`super::super::tree_from_source`].

use serde_json::{Value, json};

use super::super::tree_from_source;
use super::{MoneyAct, money_refusal, money_signal};

const W: f64 = 390.0;
const H: f64 = 844.0;

fn el(kind: &str, label: &str, rect: [f64; 4], children: Vec<Value>) -> Value {
    json!({
        "type": format!("XCUIElementType{kind}"),
        "label": label,
        "rect": { "x": rect[0], "y": rect[1], "width": rect[2], "height": rect[3] },
        "children": children,
    })
}

fn field(kind: &str, value: &str, rect: [f64; 4]) -> Value {
    let mut node = el(kind, "", rect, Vec::new());
    node["value"] = json!(value);
    node
}

/// An app whose own name carries a payment word: the name states nothing.
fn app(children: Vec<Value>) -> Value {
    el(
        "Application",
        "Synthetic Pay",
        [0.0, 0.0, W, H],
        vec![el("Window", "", [0.0, 0.0, W, H], children)],
    )
}

fn centre(rect: [f64; 4]) -> (f64, f64) {
    (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0)
}

fn press(source: &Value, rect: [f64; 4]) -> Option<String> {
    let (x, y) = centre(rect);
    money_refusal(
        &tree_from_source(source, Some("com.example.bank")),
        MoneyAct::Press(x, y),
    )
}

fn swipe(source: &Value, from: (f64, f64), to: (f64, f64)) -> Option<String> {
    money_refusal(
        &tree_from_source(source, Some("com.example.bank")),
        MoneyAct::Swipe { from, to },
    )
}

fn enter(source: &Value) -> Option<String> {
    money_refusal(
        &tree_from_source(source, Some("com.example.bank")),
        MoneyAct::Enter,
    )
}

/// Where each money screen draws its forward control and its 「취소」.
const GO: [f64; 4] = [20.0, 700.0, 170.0, 56.0];
const CANCEL: [f64; 4] = [200.0, 700.0, 170.0, 56.0];
const TAB_TRANSFER: [f64; 4] = [130.0, 780.0, 130.0, 64.0];
const AMOUNT_FIELD: [f64; 4] = [20.0, 180.0, 350.0, 60.0];

fn buttons(go: &str) -> Vec<Value> {
    vec![
        el("Button", go, GO, Vec::new()),
        el("Button", "취소", CANCEL, Vec::new()),
    ]
}

/// A bank's home: its balance and history in rows, and its sections in a tab
/// bar, 「이체」 among them.
fn bank_home(go: &str) -> Value {
    let mut content = vec![
        el("NavigationBar", "홈", [0.0, 44.0, W, 44.0], Vec::new()),
        el(
            "Table",
            "",
            [0.0, 140.0, W, 400.0],
            vec![el(
                "Cell",
                "입출금 통장 1,234,567원",
                [0.0, 140.0, W, 80.0],
                Vec::new(),
            )],
        ),
        el(
            "TabBar",
            "",
            [0.0, 780.0, W, 64.0],
            vec![
                el("Button", "홈", [0.0, 780.0, 130.0, 64.0], Vec::new()),
                el("Button", "이체", TAB_TRANSFER, Vec::new()),
            ],
        ),
    ];
    content.extend(buttons(go));
    app(content)
}

/// 「홍길동님에게 50,000원을 보낼까요?」: as a sheet over a screen that states no
/// money, or as a screen of its own.
fn transfer_ask(go: &str, as_sheet: bool) -> Value {
    let mut ask = vec![
        el(
            "StaticText",
            "홍길동님에게",
            [20.0, 520.0, 350.0, 30.0],
            Vec::new(),
        ),
        el(
            "StaticText",
            "50,000원을 보낼까요?",
            [20.0, 560.0, 350.0, 40.0],
            Vec::new(),
        ),
    ];
    ask.extend(buttons(go));
    if as_sheet {
        app(vec![
            el("StaticText", "친구", [20.0, 100.0, 350.0, 40.0], Vec::new()),
            el("Sheet", "", [0.0, 480.0, W, 364.0], ask),
        ])
    } else {
        app(vec![el("Other", "", [0.0, 0.0, W, H], ask)])
    }
}

/// A shop's last screen: 「Place your order」 under its total.
fn order_review(go: &str) -> Value {
    let mut content = vec![
        el(
            "StaticText",
            "Review your order",
            [20.0, 100.0, 350.0, 40.0],
            Vec::new(),
        ),
        el(
            "StaticText",
            "Order total: $23.45",
            [20.0, 160.0, 350.0, 40.0],
            Vec::new(),
        ),
    ];
    content.extend(buttons(go));
    app(content)
}

/// 재검수 t-94180 P2-2: a bottom sheet drawn as a plain view that names no
/// amount, over an amount field: the field's amount is on the screen.
fn plain_sheet_over_an_amount(go: &str) -> Value {
    let mut sheet = vec![el(
        "StaticText",
        "홍길동님에게 보낼까요?",
        [20.0, 560.0, 350.0, 40.0],
        Vec::new(),
    )];
    sheet.extend(buttons(go));
    app(vec![
        el(
            "StaticText",
            "얼마를 보낼까요?",
            [20.0, 120.0, 350.0, 40.0],
            Vec::new(),
        ),
        field("TextField", "50,000원", AMOUNT_FIELD),
        el("Other", "", [0.0, 480.0, W, 364.0], sheet),
    ])
}

/// The rows a transfer's last screen states it in: a table's cells, as UIKit
/// and SwiftUI draw them.
fn transfer_rows(go: &str) -> Value {
    let rows = [
        "받는 분 홍길동",
        "이체 금액 50,000원",
        "출금 계좌 입출금 통장",
    ]
    .into_iter()
    .zip(0_u32..)
    .map(|(words, row)| {
        let top = 100.0 + f64::from(row) * 60.0;
        el(
            "Cell",
            "",
            [0.0, top, W, 60.0],
            vec![el(
                "StaticText",
                words,
                [20.0, top + 15.0, 350.0, 30.0],
                Vec::new(),
            )],
        )
    })
    .collect();
    let mut content = vec![el("Table", "", [0.0, 100.0, W, 400.0], rows)];
    content.extend(buttons(go));
    app(content)
}

/// An amount drawn in two pieces on one line: 「50000」 and 「원」.
fn amount_in_two_pieces(go: &str) -> Value {
    let mut content = vec![el(
        "Other",
        "",
        [20.0, 200.0, 350.0, 60.0],
        vec![
            el(
                "StaticText",
                "50000",
                [20.0, 200.0, 200.0, 60.0],
                Vec::new(),
            ),
            el("StaticText", "원", [220.0, 200.0, 60.0, 60.0], Vec::new()),
        ],
    )];
    content.extend(buttons(go));
    app(content)
}

fn money_screens(go: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("a bank's home", bank_home(go)),
        ("a transfer's sheet", transfer_ask(go, true)),
        ("a transfer's own screen", transfer_ask(go, false)),
        ("a shop's last screen", order_review(go)),
        (
            "a plain sheet over an amount",
            plain_sheet_over_an_amount(go),
        ),
        ("a confirm drawn as rows", transfer_rows(go)),
        ("an amount in two pieces", amount_in_two_pieces(go)),
    ]
}

/// Every forward word a review found a way past (재검수 t-94180 P1-2), the
/// steps onward a field once opened (재검수 t-94000 P1-1), and an icon: none of
/// them backs out, so none is the agent's where money is stated.
#[test]
fn every_press_but_a_back_on_a_money_screen_is_the_persons() {
    for go in [
        "보내기",
        "확인",
        "Proceed",
        "Order now",
        "Complete",
        "Place your order",
        "다음",
        "계속하기",
        "Continue",
        "",
    ] {
        for (name, screen) in money_screens(go) {
            assert!(press(&screen, GO).is_some(), "{name}: {go:?}");
            assert!(enter(&screen).is_some(), "{name}: enter over {go:?}");
        }
    }
    let refused = press(&transfer_ask("보내기", true), GO).expect("its 「보내기」");
    assert!(refused.contains("50,000"), "{refused}");
    let home = bank_home("다음");
    assert!(press(&home, TAB_TRANSFER).is_some(), "a tab");
    assert!(
        press(&plain_sheet_over_an_amount("다음"), AMOUNT_FIELD).is_some(),
        "a field: only a field already typed into takes text here"
    );
    assert!(
        press(&home, [20.0, 600.0, 40.0, 40.0]).is_some(),
        "an empty place"
    );
}

/// 재검수 t-94180 P1-1 and P2-3: a still or short drag is a press, and a sideways
/// or slanted one may slide a slide-to-send; a long vertical one scrolls, from
/// wherever it starts.
#[test]
fn only_a_vertical_scroll_drags_on_a_money_screen() {
    let (x, y) = centre(GO);
    for (name, screen) in money_screens("보내기") {
        assert!(swipe(&screen, (x, y), (x, y)).is_some(), "{name}: still");
        assert!(
            swipe(&screen, (x, y), (x, y + 40.0)).is_some(),
            "{name}: short"
        );
        assert!(
            swipe(&screen, (340.0, 300.0), (40.0, 300.0)).is_some(),
            "{name}: sideways"
        );
        assert!(
            swipe(&screen, (40.0, 300.0), (290.0, 560.0)).is_some(),
            "{name}: slanted"
        );
        assert_eq!(
            swipe(&screen, (195.0, 600.0), (195.0, 200.0)),
            None,
            "{name}: a scroll up"
        );
        assert_eq!(
            swipe(&screen, (195.0, 200.0), (245.0, 600.0)),
            None,
            "{name}: a scroll down, a little aslant"
        );
        assert_eq!(
            swipe(&screen, (x, y), (x, y - 400.0)),
            None,
            "{name}: a scroll from the button"
        );
    }
}

/// Backing out stays open: a control whose every word is a back, a close icon,
/// and the navigation bar's back button, whose label is the screen before's
/// title. 「결제 취소」 and 「취소 확인」 say more than a back.
#[test]
fn a_back_on_a_money_screen_is_the_agents() {
    for (name, screen) in money_screens("보내기") {
        assert_eq!(press(&screen, CANCEL), None, "{name}");
    }
    let back = [0.0, 44.0, 80.0, 44.0];
    let done = [300.0, 44.0, 80.0, 44.0];
    let close = [330.0, 100.0, 44.0, 44.0];
    let screen = |back_label: &str, close_label: &str, close_image: &str| {
        let mut back_button = el("Button", back_label, back, Vec::new());
        back_button["rawIdentifier"] = json!("BackButton");
        let mut content = vec![
            el(
                "NavigationBar",
                "확인",
                [0.0, 44.0, W, 44.0],
                vec![back_button, el("Button", "완료", done, Vec::new())],
            ),
            el(
                "Button",
                close_label,
                close,
                vec![el("Image", close_image, close, Vec::new())],
            ),
        ];
        content.extend(buttons("확인"));
        content.push(el(
            "StaticText",
            "이체 금액 50,000원",
            [20.0, 200.0, 350.0, 40.0],
            Vec::new(),
        ));
        app(content)
    };
    assert_eq!(press(&screen("송금", "", "xmark"), back), None, "back");
    assert_eq!(press(&screen("Back", "닫기", ""), close), None, "닫기");
    assert_eq!(press(&screen("뒤로", "Close", ""), close), None, "Close");
    assert_eq!(press(&screen("뒤로", "", "xmark"), close), None, "xmark");
    assert_eq!(press(&screen("뒤로", "X", ""), close), None, "X");
    assert!(
        press(&screen("뒤로", "", ""), close).is_some(),
        "an icon that says nothing"
    );
    assert!(
        press(&screen("뒤로", "", ""), done).is_some(),
        "the bar's other button"
    );
    for words in ["결제 취소", "취소 확인"] {
        assert!(
            press(&screen("뒤로", words, ""), close).is_some(),
            "{words}"
        );
    }
}

/// (f) A message app with no money on the screen is untouched: its 「보내기」,
/// `enter`, a scroll and a sideways drag. A keyboard's 「$」 key beside its
/// digits states nothing, and nor does a row scrolled off the screen.
#[test]
fn a_message_with_no_money_is_sent() {
    let send = [330.0, 600.0, 50.0, 44.0];
    let screen = app(vec![
        el(
            "CollectionView",
            "",
            [0.0, 88.0, W, 500.0],
            vec![
                el(
                    "Cell",
                    "어제 50,000원 보냈어",
                    [0.0, -200.0, W, 60.0],
                    Vec::new(),
                ),
                el(
                    "Cell",
                    "내일 몇 시에 봐?",
                    [0.0, 100.0, W, 60.0],
                    Vec::new(),
                ),
            ],
        ),
        field("TextView", "7시 어때", [10.0, 600.0, 310.0, 44.0]),
        el("Button", "보내기", send, Vec::new()),
        el(
            "Keyboard",
            "",
            [0.0, 650.0, W, 194.0],
            vec![
                el("Key", "$", [0.0, 660.0, 39.0, 44.0], Vec::new()),
                el("Key", "1", [39.0, 660.0, 39.0, 44.0], Vec::new()),
            ],
        ),
    ]);
    assert_eq!(press(&screen, send), None);
    assert_eq!(enter(&screen), None);
    assert_eq!(swipe(&screen, (195.0, 500.0), (195.0, 200.0)), None);
    assert_eq!(swipe(&screen, (340.0, 300.0), (40.0, 300.0)), None);
    let (x, y) = centre(send);
    assert_eq!(swipe(&screen, (x, y), (x, y)), None);
}

/// What states money: a payment or transfer word, or an amount's shape; a Latin
/// word counts whole.
#[test]
fn money_is_a_word_or_an_amount() {
    for (shown, said) in [
        ("50,000원을 보낼까요?", "50,000원"),
        ("5만 원", "5만 원"),
        ("$12.50", "$12.50"),
        ("Pay 10 USD", "pay"),
        ("10 usd", "10 usd"),
        ("1,234", "1,234"),
        ("송금할 금액", "송금"),
        ("Slide to transfer", "transfer"),
    ] {
        assert_eq!(money_signal(shown).as_deref(), Some(said), "{shown}");
    }
    for shown in [
        "iPhone 15",
        "3시 30분",
        "2026",
        "Display & Brightness",
        "Paypal-less",
        "Book",
        "",
    ] {
        assert_eq!(money_signal(shown), None, "{shown}");
    }
}

/// Two pieces on different lines are not one amount.
#[test]
fn two_pieces_apart_are_no_amount() {
    let screen = app(vec![
        el(
            "StaticText",
            "50000",
            [20.0, 200.0, 200.0, 60.0],
            Vec::new(),
        ),
        el("StaticText", "원", [220.0, 330.0, 60.0, 60.0], Vec::new()),
        el("Button", "보내기", GO, Vec::new()),
    ]);
    assert_eq!(press(&screen, GO), None);
}

/// An amount's currency may come first as a code, and the rupee, rouble, lira,
/// dong, peso and shekel signs are amounts too.
#[test]
fn an_amount_in_any_currency_is_money() {
    for (shown, said) in [
        ("KRW 5000", "krw 5000"),
        ("USD 50", "usd 50"),
        ("₹500", "₹500"),
        ("₽ 900", "₽ 900"),
        ("₫20000", "₫20000"),
        ("₱150", "₱150"),
        ("₪40", "₪40"),
        ("₺75", "₺75"),
    ] {
        assert_eq!(money_signal(shown).as_deref(), Some(said), "{shown}");
    }
}
