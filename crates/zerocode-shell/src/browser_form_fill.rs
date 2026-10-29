//! 페이지 폼 채움의 판단 반쪽 (t-79331 ③).
//!
//! 페이지 쪽 손은 `ui/browser-form-fill.js` 하나다: 폼의 칸 종류를 찾고
//! (`detect`), 받은 값을 빈 칸에 쓴다(`fill`). 여기는 그 사이의 판단이다 —
//! 저장된 주소 가운데 무엇을 이 페이지에 내밀지, 칸마다 어떤 값을 줄지.
//!
//! 창에 올라가는 것은 [`FillOffer`](고를 때 읽는 한 줄들)뿐이고, 채운 뒤
//! 돌아오는 것은 [`FillReport`](몇 칸에 들어갔는지)뿐이다. 값은 채움 식에 실려
//! 판의 렌더러로만 간다. 결제 카드는 이 길에 없다 — 가져오지 않았고
//! (`browser_profile_data`), 페이지 쪽도 카드 칸을 칸 종류로 보지 않는다.
//!
//! 주소와 입력값은 따로 고른다. 주소 단추는 주소 칸만 채운다. 입력값은 칸
//! 이름으로만 이어지고 크롬의 표에는 어느 사이트에서 쳤는지가 없어서, 다른
//! 사이트에서 친 값이 이 페이지가 고른 이름의 칸에 들어갈 수 있다 — 그래서
//! 입력값은 사람이 그 값 하나를 눌렀을 때 그 이름의 칸에만 간다.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::browser_profile_data::{AddressBook, AutofillEntry, SavedAddress};

/// 페이지 쪽 스크립트 — 판의 문서에서 한 식으로 평가된다.
pub const FORM_FILL_JS: &str = include_str!("../../../ui/browser-form-fill.js");

/// 주소를 내밀려면 폼에 서로 다른 칸 종류가 이만큼은 있어야 한다 — 검색칸
/// 하나, 뉴스레터의 이메일칸 하나, 아이디·비밀번호뿐인 로그인 폼에는 내밀지
/// 않는다.
pub const MIN_KINDS: usize = 2;

/// 고르는 줄에 이름과 함께 서는 값 — 주소의 첫 줄, 없으면 이메일.
const LABEL_SECOND: [&str; 3] = ["address-line1", "street-address", "email"];

/// 한 폼에 내미는 입력값 단추의 상한 — 칸을 많이 세운 페이지가 띠를 사람의
/// 지난 입력으로 채우지 못하게.
pub const MAX_ENTRY_OFFERS: usize = 3;

/// 페이지 쪽 `detect`의 답. 칸의 값은 읽지 않는다.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PageForm {
    /// 폼이 가진 칸 종류(HTML autocomplete 낱말).
    pub kinds: Vec<String>,
    /// 종류를 모르는 빈 글자 칸들의 `name` — 입력값이 이 이름으로 채워진다.
    pub names: Vec<String>,
}

/// 창이 띠에 그리는 한 주소.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressCandidate {
    pub id: String,
    /// 사람이 고를 때 읽는 한 줄 — 이름과 주소 첫 줄(또는 이메일).
    pub label: String,
    /// 이 주소가 이 폼에서 채울 수 있는 칸 종류의 수.
    pub kinds: usize,
}

/// 띠에 따로 서는 입력값 하나 — 그 칸 이름에 가장 많이 쓴 값.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryCandidate {
    /// 페이지의 칸 이름(`name` 속성) — 누르면 이 이름의 빈 칸에만 간다.
    pub name: String,
    /// 사람이 고를 때 읽는 그 값의 첫 줄.
    pub label: String,
}

/// 이 페이지의 폼에 내미는 것 — 주소들과, 따로 고르는 입력값들.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillOffer {
    pub addresses: Vec<AddressCandidate>,
    pub entries: Vec<EntryCandidate>,
}

/// 채움의 결과 — 몇 칸에 들어갔는지뿐이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FillReport {
    /// 주소로 채운 칸.
    pub fields: usize,
    /// 입력값으로 채운 칸.
    pub entries: usize,
}

fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|line| !line.is_empty())
}

/// 저장된 주소가 한 칸 종류에 줄 값. 저장된 낱말이 그대로 있으면 그것이고,
/// 없으면 같은 뜻을 다른 모양으로 든 낱말에서 짓는다: 이름은 이름 조각들에서,
/// 주소 줄은 여러 줄 주소에서(또는 그 반대로). 폼에 둘째 줄 칸이 따로 있으면
/// 여러 줄 주소 칸에는 첫 줄만 간다 — 둘째 줄이 두 번 쓰이지 않게.
fn value_for(address: &SavedAddress, kind: &str, kinds: &BTreeSet<&str>) -> Option<String> {
    let field = |key: &str| address.fields.get(key).map(String::as_str);
    let street = field("street-address");
    let joined = |parts: &[&str], with: &str| {
        let held: Vec<&str> = parts.iter().filter_map(|key| field(*key)).collect();
        (!held.is_empty()).then(|| held.join(with))
    };
    match kind {
        "name" => field("name")
            .map(str::to_string)
            .or_else(|| joined(&["given-name", "additional-name", "family-name"], " ")),
        "street-address" if kinds.contains("address-line2") => field("address-line1")
            .or_else(|| street.and_then(|text| lines(text).next()))
            .map(str::to_string),
        "street-address" => street
            .map(str::to_string)
            .or_else(|| joined(&["address-line1", "address-line2"], "\n")),
        "address-line1" => field("address-line1")
            .or_else(|| street.and_then(|text| lines(text).next()))
            .map(str::to_string),
        "address-line2" => field("address-line2").map(str::to_string).or_else(|| {
            let rest: Vec<&str> = street.map(|text| lines(text).skip(1).collect())?;
            (!rest.is_empty()).then(|| rest.join(" "))
        }),
        _ => field(kind).map(str::to_string),
    }
}

/// 이 폼의 칸 종류마다 저장된 주소가 줄 값.
#[must_use]
pub fn fill_values(address: &SavedAddress, kinds: &[String]) -> BTreeMap<String, String> {
    let wanted: BTreeSet<&str> = kinds.iter().map(String::as_str).collect();
    wanted
        .iter()
        .filter_map(|kind| Some(((*kind).to_string(), value_for(address, kind, &wanted)?)))
        .collect()
}

/// 한 칸 이름에 가장 많이 쓴 입력값. 같은 이름의 입력값은 많이 쓴 것이 앞에
/// 서 있다(`browser_profile_data`).
fn most_used<'a>(book: &'a AddressBook, name: &str) -> Option<&'a AutofillEntry> {
    book.entries.iter().find(|entry| entry.name == name)
}

/// 종류를 모르는 칸마다 가장 많이 쓴 입력값.
#[must_use]
pub fn entry_values(book: &AddressBook, names: &[String]) -> BTreeMap<String, String> {
    names
        .iter()
        .filter_map(|name| Some((name.clone(), most_used(book, name)?.value.clone())))
        .collect()
}

fn label_of(address: &SavedAddress, kinds: &BTreeSet<&str>) -> String {
    let name = value_for(address, "name", kinds);
    let second = LABEL_SECOND
        .iter()
        .find_map(|kind| value_for(address, kind, &BTreeSet::new()))
        .map(|text| lines(&text).next().unwrap_or_default().to_string());
    [name, second]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 이 페이지의 폼에 내밀 것. 폼의 칸 종류가 [`MIN_KINDS`]보다 적으면
/// 아무것도 내밀지 않고, 이 폼에서 한 칸도 못 채우는 주소는 빠진다. 많이 쓴
/// 주소가 앞이다(저장 순서). 입력값은 주소를 내미는 폼에서만, 종류를 모르는
/// 빈 칸의 이름마다 가장 많이 쓴 값 하나를 [`MAX_ENTRY_OFFERS`]개까지 내민다.
#[must_use]
pub fn offer(book: &AddressBook, form: &PageForm) -> FillOffer {
    let kinds: BTreeSet<&str> = form.kinds.iter().map(String::as_str).collect();
    if kinds.len() < MIN_KINDS {
        return FillOffer::default();
    }
    let addresses: Vec<AddressCandidate> = book
        .addresses
        .iter()
        .filter_map(|address| {
            let filled = fill_values(address, &form.kinds).len();
            (filled > 0).then(|| AddressCandidate {
                id: address.id.clone(),
                label: label_of(address, &kinds),
                kinds: filled,
            })
        })
        .collect();
    if addresses.is_empty() {
        return FillOffer::default();
    }
    let entries = form
        .names
        .iter()
        .filter_map(|name| {
            let entry = most_used(book, name)?;
            Some(EntryCandidate {
                name: name.clone(),
                label: lines(&entry.value).next().unwrap_or_default().to_string(),
            })
        })
        .take(MAX_ENTRY_OFFERS)
        .collect();
    FillOffer { addresses, entries }
}

/// 폼 탐지 식 — 값 인자가 없다.
#[must_use]
pub fn detect_script() -> String {
    format!("({FORM_FILL_JS})(\"detect\", null)")
}

/// 채움 식. 값은 JSON 상수로 한 번 실려 이 식과 함께 렌더러에 간다. 반환값에는
/// 칸 수만 있으므로, 식의 결과를 읽는 쪽은 값을 만나지 않는다.
#[must_use]
pub fn fill_script(
    values: &BTreeMap<String, String>,
    entries: &BTreeMap<String, String>,
) -> String {
    let args = serde_json::json!({ "values": values, "entries": entries });
    format!("({FORM_FILL_JS})(\"fill\", {args})")
}

/// 채움 식의 답에서 칸 수만 꺼낸다. 그 밖의 것은 읽지 않는다.
#[must_use]
pub fn fill_report(page_answer: &serde_json::Value) -> FillReport {
    let count = |key: &str| {
        page_answer
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .map_or(0, |n| usize::try_from(n).unwrap_or(0))
    };
    FillReport {
        fields: count("fields"),
        entries: count("entries"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(id: &str, pairs: &[(&str, &str)]) -> SavedAddress {
        SavedAddress {
            id: id.into(),
            fields: pairs
                .iter()
                .map(|(kind, value)| ((*kind).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    fn form(kinds: &[&str]) -> PageForm {
        PageForm {
            kinds: kinds.iter().map(|kind| (*kind).to_string()).collect(),
            names: Vec::new(),
        }
    }

    fn full() -> SavedAddress {
        address(
            "a",
            &[
                ("given-name", "[합성-이름]"),
                ("family-name", "[합성-성]"),
                ("email", "[합성-이메일]"),
                ("tel", "[합성-전화]"),
                ("street-address", "[합성-주소-1줄]\n[합성-주소-2줄]"),
                ("postal-code", "[합성-우편번호]"),
            ],
        )
    }

    /// 폼에 칸 종류가 둘 이상이어야 주소를 내민다 — 검색칸·이메일칸 하나·
    /// 로그인 폼에는 띠가 서지 않는다.
    #[test]
    fn a_form_with_fewer_than_two_kinds_is_offered_nothing() {
        let book = AddressBook {
            addresses: vec![full()],
            entries: Vec::new(),
        };
        assert_eq!(offer(&book, &form(&[])), FillOffer::default());
        assert_eq!(offer(&book, &form(&["email"])), FillOffer::default());
        let shown = offer(&book, &form(&["name", "postal-code", "street-address"])).addresses;
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].kinds, 3);
        assert_eq!(shown[0].label, "[합성-이름] [합성-성] · [합성-주소-1줄]");
    }

    /// 이 폼에서 한 칸도 못 채우는 주소는 빠지고, 저장 순서(많이 쓴 순)는 지킨다.
    #[test]
    fn an_address_that_fills_nothing_here_is_left_out() {
        let book = AddressBook {
            addresses: vec![
                address("only-org", &[("organization", "[합성-회사]")]),
                full(),
                address("mail", &[("email", "[합성-이메일-2]")]),
            ],
            entries: Vec::new(),
        };
        let shown = offer(&book, &form(&["email", "tel"])).addresses;
        assert_eq!(
            shown.iter().map(|one| one.id.as_str()).collect::<Vec<_>>(),
            ["a", "mail"]
        );
        assert_eq!(shown[1].label, "[합성-이메일-2]");
    }

    /// 같은 뜻의 다른 모양: 이름 조각에서 이름을, 여러 줄 주소에서 줄을 짓고,
    /// 둘째 줄 칸이 따로 있으면 여러 줄 주소 칸에는 첫 줄만 간다.
    #[test]
    fn values_are_built_from_the_shape_the_address_was_kept_in() {
        let one = full();
        let values = fill_values(
            &one,
            &["name", "address-line1", "address-line2", "country"].map(String::from),
        );
        assert_eq!(values["name"], "[합성-이름] [합성-성]");
        assert_eq!(values["address-line1"], "[합성-주소-1줄]");
        assert_eq!(values["address-line2"], "[합성-주소-2줄]");
        assert!(!values.contains_key("country"), "a value was made up");

        let split = fill_values(&one, &["street-address", "address-line2"].map(String::from));
        assert_eq!(split["street-address"], "[합성-주소-1줄]");
        assert_eq!(split["address-line2"], "[합성-주소-2줄]");
        let whole = fill_values(&one, &["street-address"].map(String::from));
        assert_eq!(whole["street-address"], "[합성-주소-1줄]\n[합성-주소-2줄]");

        let kept_in_lines = address(
            "b",
            &[
                ("address-line1", "[합성-1줄]"),
                ("address-line2", "[합성-2줄]"),
            ],
        );
        assert_eq!(
            fill_values(&kept_in_lines, &["street-address"].map(String::from))["street-address"],
            "[합성-1줄]\n[합성-2줄]"
        );
    }

    /// 입력값은 칸 이름이 같은 것 가운데 가장 많이 쓴 값이다.
    #[test]
    fn an_entry_fills_the_field_of_its_own_name_with_the_most_used_value() {
        let book = AddressBook {
            addresses: Vec::new(),
            entries: vec![
                AutofillEntry {
                    name: "nickname".into(),
                    value: "[합성-별명-많이]".into(),
                    count: 7,
                },
                AutofillEntry {
                    name: "nickname".into(),
                    value: "[합성-별명-적게]".into(),
                    count: 1,
                },
            ],
        };
        let values = entry_values(&book, &["nickname".into(), "q".into()]);
        assert_eq!(values.len(), 1);
        assert_eq!(values["nickname"], "[합성-별명-많이]");
    }

    /// 입력값은 주소 단추에 섞이지 않고 따로 선다: 주소를 내미는 폼에서만,
    /// 저장된 입력값이 있는 칸 이름마다 한 줄로, 상한까지. 주소가 없는 폼에는
    /// 입력값도 내밀지 않는다.
    #[test]
    fn past_entries_are_offered_apart_from_the_addresses() {
        let entry = |name: &str, value: &str| AutofillEntry {
            name: name.into(),
            value: value.into(),
            count: 1,
        };
        let book = AddressBook {
            addresses: vec![full()],
            entries: vec![
                entry("memo", "[합성-메모-1줄]\n[합성-메모-2줄]"),
                entry("nickname", "[합성-별명]"),
                entry("a", "[합성-가]"),
                entry("b", "[합성-나]"),
            ],
        };
        let names = |list: &[&str]| list.iter().map(|name| (*name).to_string()).collect();
        let shown = offer(
            &book,
            &PageForm {
                kinds: vec!["name".into(), "postal-code".into()],
                names: names(&["memo", "q", "nickname", "a", "b"]),
            },
        );
        assert_eq!(shown.addresses.len(), 1);
        // 페이지의 칸 순서대로, 저장된 입력값이 없는 「q」는 빠지고, 상한에서 멈춘다.
        assert_eq!(
            shown
                .entries
                .iter()
                .map(|one| one.name.as_str())
                .collect::<Vec<_>>(),
            ["memo", "nickname", "a"]
        );
        assert_eq!(MAX_ENTRY_OFFERS, 3);
        assert_eq!(shown.entries[0].label, "[합성-메모-1줄]");
        let wire = serde_json::to_value(&shown).expect("wire");
        assert!(wire["addresses"].is_array() && wire["entries"][0]["name"].is_string());

        let no_address = AddressBook {
            addresses: Vec::new(),
            entries: book.entries.clone(),
        };
        assert_eq!(
            offer(
                &no_address,
                &PageForm {
                    kinds: vec!["name".into(), "postal-code".into()],
                    names: names(&["memo"]),
                },
            ),
            FillOffer::default()
        );
    }

    /// 채움 식은 값을 싣고 가고, 탐지 식은 아무 값도 싣지 않으며, 돌아오는
    /// 답에서는 칸 수만 꺼낸다.
    #[test]
    fn the_fill_script_carries_values_in_and_the_report_counts_only() {
        let values = fill_values(&full(), &["postal-code", "tel"].map(String::from));
        let script = fill_script(&values, &BTreeMap::new());
        assert!(
            script.contains("[합성-우편번호]") && script.ends_with("})"),
            "{script}"
        );
        assert!(detect_script().ends_with("(\"detect\", null)"));
        let report = fill_report(&serde_json::json!({
            "fields": 2, "entries": 1, "values": { "tel": "[합성-전화]" }
        }));
        assert_eq!(
            report,
            FillReport {
                fields: 2,
                entries: 1
            }
        );
        let said = serde_json::to_string(&report).expect("json");
        assert!(!said.contains("합성"), "{said}");
    }
}
