-- A synthetic Chrome "Web Data" at schema version 134 (addresses live in
-- addresses + address_type_tokens). Every value is a marker; no real person,
-- card or address is in here. The card-number-shaped autofill value the test
-- needs is written by the test at run time, never as a literal in this file.
CREATE TABLE meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
INSERT INTO meta VALUES('version', '134');
INSERT INTO meta VALUES('last_compatible_version', '134');

CREATE TABLE autofill (name VARCHAR, value VARCHAR, value_lower VARCHAR,
  date_created INTEGER DEFAULT 0, date_last_used INTEGER DEFAULT 0,
  count INTEGER DEFAULT 1, PRIMARY KEY (name, value));
INSERT INTO autofill VALUES('nickname', '[합성-별명]', '[합성-별명]', 1, 20, 7);
INSERT INTO autofill VALUES('company', '[합성-회사]', '[합성-회사]', 1, 10, 3);
INSERT INTO autofill VALUES('card_cvc', '[합성-보안코드]', '[합성-보안코드]', 1, 30, 9);
INSERT INTO autofill VALUES('password', '[합성-비밀번호]', '[합성-비밀번호]', 1, 30, 8);
INSERT INTO autofill VALUES('empty', '', '', 1, 30, 8);

CREATE TABLE addresses (guid VARCHAR PRIMARY KEY, record_type INTEGER,
  use_count INTEGER NOT NULL DEFAULT 0, use_date INTEGER NOT NULL DEFAULT 0,
  date_modified INTEGER NOT NULL DEFAULT 0, language_code VARCHAR, label VARCHAR,
  initial_creator_id INTEGER DEFAULT 0, last_modifier_id INTEGER DEFAULT 0);
INSERT INTO addresses VALUES('00000000-0000-4000-8000-000000000001', 0, 2, 100, 100, 'ko', '', 0, 0);
INSERT INTO addresses VALUES('00000000-0000-4000-8000-000000000002', 0, 9, 200, 200, 'ko', '', 0, 0);

CREATE TABLE address_type_tokens (guid VARCHAR, type INTEGER, value VARCHAR,
  verification_status INTEGER DEFAULT 0, observations BLOB, PRIMARY KEY (guid, type));
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000001', 7, '[합성-이름-가]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000001', 9, '[합성-이메일-가]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 7, '[합성-이름-나]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 3, '[합성-이름]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 5, '[합성-성]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 9, '[합성-이메일-나]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 14, '[합성-전화]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 60, '[합성-회사]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 77, '[합성-주소-1줄]' || char(10) || '[합성-주소-2줄]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 33, '[합성-도시]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 34, '[합성-시도]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 35, '[합성-우편번호]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 36, 'KR', 0, NULL);
-- A type the import does not carry (outside its table) and an empty value.
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 81, '[합성-동네]', 0, NULL);
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-000000000002', 31, '', 0, NULL);
-- A token whose address row is gone.
INSERT INTO address_type_tokens VALUES('00000000-0000-4000-8000-0000000000ff', 7, '[합성-지워진-이름]', 0, NULL);

CREATE TABLE credit_cards (guid VARCHAR PRIMARY KEY, name_on_card VARCHAR,
  expiration_month INTEGER, expiration_year INTEGER, card_number_encrypted BLOB,
  date_modified INTEGER NOT NULL DEFAULT 0, origin VARCHAR DEFAULT '',
  use_count INTEGER NOT NULL DEFAULT 0, use_date INTEGER NOT NULL DEFAULT 0,
  billing_address_id VARCHAR, nickname VARCHAR);
INSERT INTO credit_cards VALUES('20000000-0000-4000-8000-000000000001', '[합성-카드-이름]', 1, 2030, X'', 0, '', 0, 0, '', '');
INSERT INTO credit_cards VALUES('20000000-0000-4000-8000-000000000002', '[합성-카드-이름]', 2, 2031, X'', 0, '', 0, 0, '', '');
