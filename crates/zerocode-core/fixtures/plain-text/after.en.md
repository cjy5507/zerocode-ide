# Retry interval report

The retry count fell from 1,204 to 312 per day.
The judgment ran late, so callers retried often. We changed the sweep interval from 30 s to 90 s.
The worker checked the settings and found no fault. Run the full test suite when the build queue has room.

## What was checked

- Processing time fell from 7.4 s to 2.1 s.
- We have not yet checked low-spec machines.
