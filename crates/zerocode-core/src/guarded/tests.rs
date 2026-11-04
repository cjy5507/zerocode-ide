//! One table of the controls a press cannot take back, and its two readers.

use super::*;

/// The words the brief names for a destructive control, in either language
/// and inside a legend line as a screen question offers it, read as
/// destructive; a control that saves, opens or moves on reads as plain.
#[test]
fn a_control_that_pays_deletes_or_sends_is_destructive_and_the_rest_is_plain() {
    for destructive in [
        "delete",
        "Remove item",
        "Pay now",
        "Send",
        "Submit",
        "Confirm order",
        "Transfer",
        "결제하기",
        "계정 삭제",
        "메시지 전송",
        "주문 확정",
        "3 button 모두 삭제 @160,40",
    ] {
        assert_eq!(
            kind_of(destructive),
            ControlKind::Destructive,
            "{destructive}"
        );
    }
    for plain in [
        "저장",
        "다음",
        "Open settings",
        "Cancel",
        "1 button 닫기 @120,40",
    ] {
        assert_eq!(kind_of(plain), ControlKind::Plain, "{plain}");
    }
    assert_eq!(
        (ControlKind::Destructive.word(), ControlKind::Plain.word()),
        ("destructive", "plain")
    );
}

/// One table: every word the confirmation gate asks a person about is a
/// destructive control to a walk, and the commit row the walk adds never
/// makes the gate ask — a send or a submit asks nobody today, as before.
#[test]
fn the_gate_and_the_walk_read_one_table_and_the_commit_row_asks_no_person() {
    for kind in ConfirmKind::ALL {
        for word in kind.words() {
            assert_eq!(kind_of(word), ControlKind::Destructive, "{word}");
        }
    }
    for word in COMMIT_WORDS {
        assert_eq!(kind_of(word), ControlKind::Destructive, "{word}");
        assert_eq!(
            confirm_kind_of(word),
            None,
            "{word} would make the gate ask"
        );
    }
    assert_eq!(confirm_kind_of("결제하기"), Some(ConfirmKind::Payment));
    assert_eq!(confirm_kind_of("Send money"), Some(ConfirmKind::Transfer));
}

/// 재검수 t-93881 P2-1 and t-94000 P2-2: a bill paid, a balance topped up or
/// filled, a subscription, a gift of money, a withdrawal and a deposit ask a
/// person like a payment and a transfer; an application sent is a commit.
#[test]
fn a_bill_a_top_up_a_withdrawal_and_an_application_are_held() {
    for (label, kind) in [
        ("납부하기", ConfirmKind::Payment),
        ("충전", ConfirmKind::Payment),
        ("Top up", ConfirmKind::Payment),
        ("후원하기", ConfirmKind::Payment),
        ("출금하기", ConfirmKind::Transfer),
        ("Withdraw", ConfirmKind::Transfer),
        ("입금하기", ConfirmKind::Transfer),
        ("Deposit", ConfirmKind::Transfer),
        ("채우기", ConfirmKind::Payment),
        ("구독하기", ConfirmKind::Payment),
        ("Subscribe", ConfirmKind::Payment),
        ("지불하기", ConfirmKind::Payment),
    ] {
        assert_eq!(confirm_kind_of(label), Some(kind), "{label}");
    }
    assert_eq!(kind_of("대출 신청하기"), ControlKind::Destructive);
    assert_eq!(confirm_kind_of("대출 신청하기"), None);
}

/// The list a page script is handed is the table's rows whole and in order:
/// the gate's three kinds first, each in the gate's own order, the commit row
/// the walk adds last — so a word added to any row reaches every reader.
#[test]
fn the_held_rows_are_the_tables_four_rows_in_their_order() {
    let words: Vec<&str> = HELD_ROWS.concat();
    let gate: Vec<&str> = ConfirmKind::ALL
        .into_iter()
        .flat_map(|kind| kind.words().iter().copied())
        .collect();
    assert_eq!(words[..gate.len()], gate[..], "the gate's rows come first");
    assert_eq!(
        words[gate.len()..],
        *COMMIT_WORDS,
        "and the commit row last"
    );
    for word in &words {
        assert_eq!(kind_of(word), ControlKind::Destructive, "{word}");
    }
}
