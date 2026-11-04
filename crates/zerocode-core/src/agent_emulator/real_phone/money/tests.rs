//! 검수 t-93698 P2-2: the press that sends money is the person's, and only it.
//! Every screen here is in the shape WebDriverAgent answers (`XCUIElementType…`,
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

const NEXT: [f64; 4] = [20.0, 560.0, 350.0, 50.0];
const TAB_HOME: [f64; 4] = [0.0, 780.0, 130.0, 64.0];
const TAB_TRANSFER: [f64; 4] = [130.0, 780.0, 130.0, 64.0];
const TAB_ALL: [f64; 4] = [260.0, 780.0, 130.0, 64.0];

/// A bank's home: its balance and history in rows, a 「다음」 on a banner, and
/// its sections in a tab bar, 「이체」 among them.
fn bank_home() -> Vec<Value> {
    vec![
        el("NavigationBar", "홈", [0.0, 44.0, W, 44.0], Vec::new()),
        el(
            "StaticText",
            "내 계좌",
            [20.0, 100.0, 200.0, 30.0],
            Vec::new(),
        ),
        el(
            "Table",
            "",
            [0.0, 140.0, W, 400.0],
            vec![
                el(
                    "Cell",
                    "입출금 통장 1,234,567원",
                    [0.0, 140.0, W, 80.0],
                    vec![el("Button", "이체", [300.0, 160.0, 70.0, 40.0], Vec::new())],
                ),
                el(
                    "Cell",
                    "카드 결제 30,000원",
                    [0.0, 220.0, W, 80.0],
                    Vec::new(),
                ),
            ],
        ),
        el("Button", "다음", NEXT, Vec::new()),
        el(
            "TabBar",
            "",
            [0.0, 780.0, W, 64.0],
            vec![
                el("Button", "홈", TAB_HOME, Vec::new()),
                el("Button", "이체", TAB_TRANSFER, Vec::new()),
                el("Button", "전체", TAB_ALL, Vec::new()),
            ],
        ),
    ]
}

/// (a) A bank's home is the agent's: scrolling, moving between tabs, a 「다음」
/// and `enter`, whatever its rows and tabs say of transfers and payments.
#[test]
fn a_banks_home_scrolls_moves_and_goes_on() {
    let home = app(bank_home());
    assert_eq!(
        swipe(&home, (195.0, 400.0), (195.0, 160.0)),
        None,
        "a scroll"
    );
    assert_eq!(
        swipe(&home, (300.0, 250.0), (60.0, 250.0)),
        None,
        "a row's swipe"
    );
    assert_eq!(press(&home, TAB_ALL), None, "a tab");
    assert_eq!(press(&home, TAB_TRANSFER), None, "the transfer tab moves");
    assert_eq!(press(&home, NEXT), None, "a 「다음」 with no money stated");
    assert_eq!(enter(&home), None);
}

const SEND: [f64; 4] = [20.0, 740.0, 170.0, 56.0];
const CANCEL: [f64; 4] = [200.0, 740.0, 170.0, 56.0];

/// (b) 「홍길동님에게 50,000원을 보낼까요?」 and 「보내기」: no money word is on it,
/// its amount is. Over the bank's home as a sheet, and as a screen of its own.
#[test]
fn a_sheet_that_asks_to_send_an_amount_is_the_persons() {
    let ask = || {
        vec![
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
            el("Button", "보내기", SEND, Vec::new()),
            el("Button", "취소", CANCEL, Vec::new()),
        ]
    };
    let mut over_home = bank_home();
    over_home.push(el("Sheet", "", [0.0, 480.0, W, 364.0], ask()));
    let own_screen = vec![el("Other", "", [0.0, 0.0, W, H], ask())];
    for screen in [app(over_home), app(own_screen)] {
        let refused = press(&screen, SEND).expect("its 「보내기」 is the person's");
        assert!(refused.contains("50,000"), "{refused}");
        assert!(enter(&screen).is_some(), "enter over it");
        assert_eq!(press(&screen, CANCEL), None, "backing out");
        assert_eq!(
            swipe(&screen, (195.0, 600.0), (195.0, 300.0)),
            None,
            "a scroll"
        );
    }
}

/// (c) A confirm that is only an icon — a check, an arrow — on a money sheet is
/// the person's; an icon that says it closes is not.
#[test]
fn an_icon_only_yes_on_a_money_sheet_is_the_persons() {
    let check = [300.0, 700.0, 60.0, 60.0];
    let close = [20.0, 500.0, 44.0, 44.0];
    let screen = app(vec![el(
        "Sheet",
        "",
        [0.0, 480.0, W, 364.0],
        vec![
            el(
                "StaticText",
                "₩50,000 이체",
                [20.0, 560.0, 350.0, 40.0],
                Vec::new(),
            ),
            el(
                "Button",
                "",
                check,
                vec![el("Image", "", check, Vec::new())],
            ),
            el(
                "Button",
                "닫기",
                close,
                vec![el("Image", "", close, Vec::new())],
            ),
        ],
    )]);
    assert!(press(&screen, check).is_some(), "an icon's yes");
    assert_eq!(press(&screen, close), None, "an icon that closes");
    assert_eq!(
        press(&screen, [20.0, 780.0, 40.0, 40.0]),
        None,
        "an empty place"
    );
    let plain_icon = [300.0, 620.0, 60.0, 60.0];
    let mut with_plain = screen.clone();
    with_plain["children"][0]["children"][0]["children"]
        .as_array_mut()
        .expect("the sheet's children")
        .push(el("Other", "", plain_icon, Vec::new()));
    assert!(
        press(&with_plain, plain_icon).is_some(),
        "a plain view drawn as an icon"
    );
}

/// (d) 「밀어서 확인」: a slider on a money sheet sends by a drag, whichever way.
/// A drag that starts elsewhere on the sheet and runs down scrolls.
#[test]
fn a_slide_to_confirm_is_the_persons() {
    let slider = [20.0, 720.0, 350.0, 60.0];
    let custom = [20.0, 640.0, 350.0, 60.0];
    let screen = app(vec![el(
        "Sheet",
        "",
        [0.0, 480.0, W, 364.0],
        vec![
            el(
                "StaticText",
                "김철수 30,000원",
                [20.0, 520.0, 350.0, 40.0],
                Vec::new(),
            ),
            el("Slider", "밀어서 확인", slider, Vec::new()),
            el("Other", "밀어서 송금", custom, Vec::new()),
        ],
    )]);
    let (x, y) = centre(slider);
    assert!(swipe(&screen, (40.0, y), (360.0, y)).is_some(), "sideways");
    assert!(swipe(&screen, (x, y), (x, y - 200.0)).is_some(), "upward");
    let (_, cy) = centre(custom);
    assert!(
        swipe(&screen, (40.0, cy), (360.0, cy)).is_some(),
        "a slider that is a plain view"
    );
    assert_eq!(
        swipe(&screen, (195.0, 540.0), (195.0, 800.0)),
        None,
        "a scroll"
    );
}

/// (e) A shop's list: its rows' 「구매하기」 and prices say nothing of a sheet, so
/// it scrolls, and `enter` is not refused for them.
#[test]
fn a_shops_list_scrolls() {
    let screen = app(vec![
        el(
            "StaticText",
            "추천 상품",
            [20.0, 100.0, 200.0, 30.0],
            Vec::new(),
        ),
        el(
            "CollectionView",
            "",
            [0.0, 140.0, W, 640.0],
            vec![el(
                "Cell",
                "운동화 89,000원",
                [0.0, 140.0, W, 120.0],
                vec![el(
                    "Button",
                    "구매하기",
                    [260.0, 200.0, 110.0, 44.0],
                    Vec::new(),
                )],
            )],
        ),
    ]);
    assert_eq!(swipe(&screen, (195.0, 600.0), (195.0, 200.0)), None);
    assert_eq!(enter(&screen), None);
}

/// A conversation: its bubbles are rows and the typed line is a field, so
/// 「보내기」 sends the message, however the conversation speaks of money.
#[test]
fn a_message_about_money_is_sent() {
    let send = [330.0, 780.0, 50.0, 44.0];
    let screen = app(vec![
        el(
            "CollectionView",
            "",
            [0.0, 88.0, W, 680.0],
            vec![
                el("Cell", "결제 완료됐어요", [0.0, 100.0, W, 60.0], Vec::new()),
                el("Cell", "3만원 송금했어", [0.0, 170.0, W, 60.0], Vec::new()),
            ],
        ),
        field("TextView", "5만원 보내줘", [10.0, 780.0, 310.0, 44.0]),
        el("Button", "보내기", send, Vec::new()),
    ]);
    assert_eq!(press(&screen, send), None);
    assert_eq!(enter(&screen), None);
}

/// An amount being typed is the agent's: the field's own text states nothing, so
/// the keypad and 「다음」 go on.
#[test]
fn typing_an_amount_and_going_on_is_the_agents() {
    let key = [20.0, 600.0, 110.0, 60.0];
    let screen = app(vec![
        el(
            "StaticText",
            "얼마를 보낼까요?",
            [20.0, 120.0, 350.0, 40.0],
            Vec::new(),
        ),
        field("TextField", "50,000원", [20.0, 180.0, 350.0, 60.0]),
        el("Button", "1", key, Vec::new()),
        el("Button", "다음", [20.0, 760.0, 350.0, 56.0], Vec::new()),
    ]);
    assert_eq!(press(&screen, key), None, "a digit");
    assert_eq!(press(&screen, [20.0, 760.0, 350.0, 56.0]), None, "다음");
}

/// A sheet is read on its own: a sheet that asks nothing of money is not one
/// because the screen under it shows a balance.
#[test]
fn a_sheet_is_read_by_its_own_words() {
    let yes = [20.0, 740.0, 350.0, 56.0];
    let screen = app(vec![
        el(
            "StaticText",
            "잔액 1,000,000원",
            [20.0, 100.0, 350.0, 40.0],
            Vec::new(),
        ),
        el(
            "Alert",
            "",
            [20.0, 600.0, 350.0, 220.0],
            vec![
                el(
                    "StaticText",
                    "알림을 켤까요?",
                    [40.0, 620.0, 310.0, 40.0],
                    Vec::new(),
                ),
                el("Button", "확인", yes, Vec::new()),
            ],
        ),
    ]);
    assert_eq!(press(&screen, yes), None);
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
