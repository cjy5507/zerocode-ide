//! 나라의 코드와 이름을 잇는 표 하나 (t-84200).
//!
//! 크롬은 주소의 나라를 두 글자 코드(`MY`)로, 때로는 이름으로 적어 둔다. 페이지의
//! 나라 `select`는 옵션의 값이 코드일 수도, 번호일 수도 있고 이름은 글자에만
//! 있을 수도 있다. 그래서 채울 때는 저장된 나라의 코드와 이름을 모두 페이지에
//! 건네 옵션을 값이나 글자로 맞추게 한다. 코드와 이름의 짝은 여기 한 곳에만 둔다.

/// (ISO 3166-1 alpha-2 코드, 이름들). 첫 이름이 대표 이름이고 나머지는
/// 페이지가 흔히 쓰는 다른 표기다. 사람이 쓰는 시장의 한국어 이름은 한국만 둔다.
const COUNTRIES: &[(&str, &[&str])] = &[
    ("AF", &["Afghanistan"]),
    ("AX", &["Åland Islands", "Aland Islands"]),
    ("AL", &["Albania"]),
    ("DZ", &["Algeria"]),
    ("AS", &["American Samoa"]),
    ("AD", &["Andorra"]),
    ("AO", &["Angola"]),
    ("AI", &["Anguilla"]),
    ("AQ", &["Antarctica"]),
    ("AG", &["Antigua and Barbuda"]),
    ("AR", &["Argentina"]),
    ("AM", &["Armenia"]),
    ("AW", &["Aruba"]),
    ("AU", &["Australia"]),
    ("AT", &["Austria"]),
    ("AZ", &["Azerbaijan"]),
    ("BS", &["Bahamas", "The Bahamas"]),
    ("BH", &["Bahrain"]),
    ("BD", &["Bangladesh"]),
    ("BB", &["Barbados"]),
    ("BY", &["Belarus"]),
    ("BE", &["Belgium"]),
    ("BZ", &["Belize"]),
    ("BJ", &["Benin"]),
    ("BM", &["Bermuda"]),
    ("BT", &["Bhutan"]),
    ("BO", &["Bolivia", "Bolivia, Plurinational State of"]),
    (
        "BQ",
        &["Bonaire, Sint Eustatius and Saba", "Caribbean Netherlands"],
    ),
    ("BA", &["Bosnia and Herzegovina", "Bosnia & Herzegovina"]),
    ("BW", &["Botswana"]),
    ("BV", &["Bouvet Island"]),
    ("BR", &["Brazil"]),
    ("IO", &["British Indian Ocean Territory"]),
    ("BN", &["Brunei", "Brunei Darussalam"]),
    ("BG", &["Bulgaria"]),
    ("BF", &["Burkina Faso"]),
    ("BI", &["Burundi"]),
    ("CV", &["Cabo Verde", "Cape Verde"]),
    ("KH", &["Cambodia"]),
    ("CM", &["Cameroon"]),
    ("CA", &["Canada"]),
    ("KY", &["Cayman Islands"]),
    ("CF", &["Central African Republic"]),
    ("TD", &["Chad"]),
    ("CL", &["Chile"]),
    ("CN", &["China", "People's Republic of China"]),
    ("CX", &["Christmas Island"]),
    ("CC", &["Cocos (Keeling) Islands"]),
    ("CO", &["Colombia"]),
    ("KM", &["Comoros"]),
    ("CG", &["Congo", "Republic of the Congo"]),
    (
        "CD",
        &[
            "Congo (Democratic Republic)",
            "Democratic Republic of the Congo",
            "DR Congo",
        ],
    ),
    ("CK", &["Cook Islands"]),
    ("CR", &["Costa Rica"]),
    ("CI", &["Côte d'Ivoire", "Cote d'Ivoire", "Ivory Coast"]),
    ("HR", &["Croatia"]),
    ("CU", &["Cuba"]),
    ("CW", &["Curaçao", "Curacao"]),
    ("CY", &["Cyprus"]),
    ("CZ", &["Czechia", "Czech Republic"]),
    ("DK", &["Denmark"]),
    ("DJ", &["Djibouti"]),
    ("DM", &["Dominica"]),
    ("DO", &["Dominican Republic"]),
    ("EC", &["Ecuador"]),
    ("EG", &["Egypt"]),
    ("SV", &["El Salvador"]),
    ("GQ", &["Equatorial Guinea"]),
    ("ER", &["Eritrea"]),
    ("EE", &["Estonia"]),
    ("SZ", &["Eswatini", "Swaziland"]),
    ("ET", &["Ethiopia"]),
    ("FK", &["Falkland Islands (Malvinas)", "Falkland Islands"]),
    ("FO", &["Faroe Islands"]),
    ("FJ", &["Fiji"]),
    ("FI", &["Finland"]),
    ("FR", &["France"]),
    ("GF", &["French Guiana"]),
    ("PF", &["French Polynesia"]),
    ("TF", &["French Southern Territories"]),
    ("GA", &["Gabon"]),
    ("GM", &["Gambia", "The Gambia"]),
    ("GE", &["Georgia"]),
    ("DE", &["Germany"]),
    ("GH", &["Ghana"]),
    ("GI", &["Gibraltar"]),
    ("GR", &["Greece"]),
    ("GL", &["Greenland"]),
    ("GD", &["Grenada"]),
    ("GP", &["Guadeloupe"]),
    ("GU", &["Guam"]),
    ("GT", &["Guatemala"]),
    ("GG", &["Guernsey"]),
    ("GN", &["Guinea"]),
    ("GW", &["Guinea-Bissau"]),
    ("GY", &["Guyana"]),
    ("HT", &["Haiti"]),
    ("HM", &["Heard Island and McDonald Islands"]),
    ("VA", &["Holy See", "Vatican City"]),
    ("HN", &["Honduras"]),
    ("HK", &["Hong Kong", "Hong Kong SAR China"]),
    ("HU", &["Hungary"]),
    ("IS", &["Iceland"]),
    ("IN", &["India"]),
    ("ID", &["Indonesia"]),
    ("IR", &["Iran", "Iran, Islamic Republic of"]),
    ("IQ", &["Iraq"]),
    ("IE", &["Ireland"]),
    ("IM", &["Isle of Man"]),
    ("IL", &["Israel"]),
    ("IT", &["Italy"]),
    ("JM", &["Jamaica"]),
    ("JP", &["Japan"]),
    ("JE", &["Jersey"]),
    ("JO", &["Jordan"]),
    ("KZ", &["Kazakhstan"]),
    ("KE", &["Kenya"]),
    ("KI", &["Kiribati"]),
    (
        "KP",
        &["North Korea", "Korea, Democratic People's Republic of"],
    ),
    (
        "KR",
        &[
            "South Korea",
            "Korea, Republic of",
            "Republic of Korea",
            "Korea",
            "대한민국",
            "한국",
        ],
    ),
    ("KW", &["Kuwait"]),
    ("KG", &["Kyrgyzstan"]),
    ("LA", &["Laos", "Lao People's Democratic Republic"]),
    ("LV", &["Latvia"]),
    ("LB", &["Lebanon"]),
    ("LS", &["Lesotho"]),
    ("LR", &["Liberia"]),
    ("LY", &["Libya"]),
    ("LI", &["Liechtenstein"]),
    ("LT", &["Lithuania"]),
    ("LU", &["Luxembourg"]),
    ("MO", &["Macao", "Macau"]),
    ("MG", &["Madagascar"]),
    ("MW", &["Malawi"]),
    ("MY", &["Malaysia"]),
    ("MV", &["Maldives"]),
    ("ML", &["Mali"]),
    ("MT", &["Malta"]),
    ("MH", &["Marshall Islands"]),
    ("MQ", &["Martinique"]),
    ("MR", &["Mauritania"]),
    ("MU", &["Mauritius"]),
    ("YT", &["Mayotte"]),
    ("MX", &["Mexico"]),
    ("FM", &["Micronesia", "Micronesia, Federated States of"]),
    ("MD", &["Moldova", "Moldova, Republic of"]),
    ("MC", &["Monaco"]),
    ("MN", &["Mongolia"]),
    ("ME", &["Montenegro"]),
    ("MS", &["Montserrat"]),
    ("MA", &["Morocco"]),
    ("MZ", &["Mozambique"]),
    ("MM", &["Myanmar", "Burma"]),
    ("NA", &["Namibia"]),
    ("NR", &["Nauru"]),
    ("NP", &["Nepal"]),
    ("NL", &["Netherlands", "The Netherlands"]),
    ("NC", &["New Caledonia"]),
    ("NZ", &["New Zealand"]),
    ("NI", &["Nicaragua"]),
    ("NE", &["Niger"]),
    ("NG", &["Nigeria"]),
    ("NU", &["Niue"]),
    ("NF", &["Norfolk Island"]),
    ("MK", &["North Macedonia", "Macedonia"]),
    ("MP", &["Northern Mariana Islands"]),
    ("NO", &["Norway"]),
    ("OM", &["Oman"]),
    ("PK", &["Pakistan"]),
    ("PW", &["Palau"]),
    (
        "PS",
        &["Palestine", "State of Palestine", "Palestinian Territories"],
    ),
    ("PA", &["Panama"]),
    ("PG", &["Papua New Guinea"]),
    ("PY", &["Paraguay"]),
    ("PE", &["Peru"]),
    ("PH", &["Philippines"]),
    ("PN", &["Pitcairn", "Pitcairn Islands"]),
    ("PL", &["Poland"]),
    ("PT", &["Portugal"]),
    ("PR", &["Puerto Rico"]),
    ("QA", &["Qatar"]),
    ("RE", &["Réunion", "Reunion"]),
    ("RO", &["Romania"]),
    ("RU", &["Russia", "Russian Federation"]),
    ("RW", &["Rwanda"]),
    ("BL", &["Saint Barthélemy", "Saint Barthelemy"]),
    (
        "SH",
        &[
            "Saint Helena, Ascension and Tristan da Cunha",
            "Saint Helena",
        ],
    ),
    ("KN", &["Saint Kitts and Nevis"]),
    ("LC", &["Saint Lucia"]),
    ("MF", &["Saint Martin (French part)", "Saint Martin"]),
    ("PM", &["Saint Pierre and Miquelon"]),
    ("VC", &["Saint Vincent and the Grenadines"]),
    ("WS", &["Samoa"]),
    ("SM", &["San Marino"]),
    ("ST", &["Sao Tome and Principe", "São Tomé and Príncipe"]),
    ("SA", &["Saudi Arabia"]),
    ("SN", &["Senegal"]),
    ("RS", &["Serbia"]),
    ("SC", &["Seychelles"]),
    ("SL", &["Sierra Leone"]),
    ("SG", &["Singapore"]),
    ("SX", &["Sint Maarten (Dutch part)", "Sint Maarten"]),
    ("SK", &["Slovakia"]),
    ("SI", &["Slovenia"]),
    ("SB", &["Solomon Islands"]),
    ("SO", &["Somalia"]),
    ("ZA", &["South Africa"]),
    ("GS", &["South Georgia and the South Sandwich Islands"]),
    ("SS", &["South Sudan"]),
    ("ES", &["Spain"]),
    ("LK", &["Sri Lanka"]),
    ("SD", &["Sudan"]),
    ("SR", &["Suriname"]),
    ("SJ", &["Svalbard and Jan Mayen"]),
    ("SE", &["Sweden"]),
    ("CH", &["Switzerland"]),
    ("SY", &["Syria", "Syrian Arab Republic"]),
    ("TW", &["Taiwan", "Taiwan, Province of China"]),
    ("TJ", &["Tajikistan"]),
    ("TZ", &["Tanzania", "Tanzania, United Republic of"]),
    ("TH", &["Thailand"]),
    ("TL", &["Timor-Leste", "East Timor"]),
    ("TG", &["Togo"]),
    ("TK", &["Tokelau"]),
    ("TO", &["Tonga"]),
    ("TT", &["Trinidad and Tobago"]),
    ("TN", &["Tunisia"]),
    ("TR", &["Türkiye", "Turkey"]),
    ("TM", &["Turkmenistan"]),
    ("TC", &["Turks and Caicos Islands"]),
    ("TV", &["Tuvalu"]),
    ("UG", &["Uganda"]),
    ("UA", &["Ukraine"]),
    ("AE", &["United Arab Emirates", "UAE"]),
    ("GB", &["United Kingdom", "UK", "Great Britain"]),
    (
        "US",
        &["United States", "United States of America", "USA", "US"],
    ),
    ("UM", &["United States Minor Outlying Islands"]),
    ("UY", &["Uruguay"]),
    ("UZ", &["Uzbekistan"]),
    ("VU", &["Vanuatu"]),
    ("VE", &["Venezuela", "Venezuela, Bolivarian Republic of"]),
    ("VN", &["Vietnam", "Viet Nam"]),
    ("VG", &["British Virgin Islands", "Virgin Islands, British"]),
    ("VI", &["U.S. Virgin Islands", "Virgin Islands, U.S."]),
    ("WF", &["Wallis and Futuna"]),
    ("EH", &["Western Sahara"]),
    ("YE", &["Yemen"]),
    ("ZM", &["Zambia"]),
    ("ZW", &["Zimbabwe"]),
    ("XK", &["Kosovo"]),
];

fn same(left: &str, right: &str) -> bool {
    left.trim().to_lowercase() == right.trim().to_lowercase()
}

/// 저장된 나라 값(코드든 이름이든)의 같은 나라 표기들 — 두 글자 코드가 맨 앞,
/// 그 뒤에 이름들. 표에 없는 값은 `None`이라 지어내지 않는다.
#[must_use]
pub fn spellings(stored: &str) -> Option<Vec<String>> {
    let (code, names) = COUNTRIES
        .iter()
        .find(|(code, names)| same(code, stored) || names.iter().any(|name| same(name, stored)))?;
    Some(
        std::iter::once(*code)
            .chain(names.iter().copied())
            .map(str::to_string)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 코드로도 이름으로도 같은 나라를 찾고, 대소문자와 앞뒤 공백은 보지 않으며,
    /// 모르는 값은 없다.
    #[test]
    fn a_code_or_a_name_finds_the_same_country() {
        let want = Some(vec!["MY".to_string(), "Malaysia".to_string()]);
        for stored in ["MY", "my", " Malaysia ", "MALAYSIA"] {
            assert_eq!(spellings(stored), want, "{stored}");
        }
        let korea = spellings("KR").expect("Korea");
        assert_eq!(korea[0], "KR");
        assert!(korea.iter().any(|name| name == "South Korea"));
        assert_eq!(spellings("Korea, Republic of"), Some(korea.clone()));
        assert_eq!(spellings("Narnia"), None);
        assert_eq!(spellings(""), None);
    }

    /// 표는 코드마다 한 줄이고, 코드는 대문자 두 글자이며, 이름은 비어 있지
    /// 않고 어느 이름도 다른 나라의 코드나 이름과 겹치지 않는다.
    #[test]
    fn the_table_has_one_row_per_code_and_no_name_in_two_countries() {
        let mut seen = std::collections::BTreeMap::new();
        for (code, names) in COUNTRIES {
            assert!(
                code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase()),
                "{code}"
            );
            assert!(
                !names.is_empty() && names.iter().all(|name| !name.trim().is_empty()),
                "{code}"
            );
            for key in std::iter::once(code).chain(names.iter()) {
                let held = seen.insert(key.to_lowercase(), *code);
                assert!(
                    held.is_none_or(|other| other == *code),
                    "{key}: {held:?} and {code}"
                );
            }
        }
        assert!(COUNTRIES.len() >= 240, "{}", COUNTRIES.len());
    }
}
