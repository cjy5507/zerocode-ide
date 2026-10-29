-- A synthetic Chrome "Web Data" from before the address tables moved to type
-- tokens (schema version 88): the address in autofill_profiles, the name,
-- email and phone in side tables. Every value is a marker.
CREATE TABLE meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
INSERT INTO meta VALUES('version', '88');

CREATE TABLE autofill_profiles (guid VARCHAR PRIMARY KEY, company_name VARCHAR,
  street_address VARCHAR, dependent_locality VARCHAR, city VARCHAR, state VARCHAR,
  zipcode VARCHAR, sorting_code VARCHAR, country_code VARCHAR,
  date_modified INTEGER NOT NULL DEFAULT 0, origin VARCHAR DEFAULT '',
  language_code VARCHAR, use_count INTEGER NOT NULL DEFAULT 0,
  use_date INTEGER NOT NULL DEFAULT 0);
INSERT INTO autofill_profiles VALUES('30000000-0000-4000-8000-000000000001', '',
  '[합성-주소-1줄]', '', '[합성-도시]', '[합성-시도]', '[합성-우편번호]', '', 'KR',
  0, '', 'ko', 4, 50);

CREATE TABLE autofill_profile_names (guid VARCHAR, first_name VARCHAR,
  middle_name VARCHAR, last_name VARCHAR, full_name VARCHAR);
INSERT INTO autofill_profile_names VALUES('30000000-0000-4000-8000-000000000001',
  '[합성-이름]', '', '[합성-성]', '');
CREATE TABLE autofill_profile_emails (guid VARCHAR, email VARCHAR);
INSERT INTO autofill_profile_emails VALUES('30000000-0000-4000-8000-000000000001', '[합성-이메일]');
CREATE TABLE autofill_profile_phones (guid VARCHAR, number VARCHAR);
INSERT INTO autofill_profile_phones VALUES('30000000-0000-4000-8000-000000000001', '[합성-전화]');
