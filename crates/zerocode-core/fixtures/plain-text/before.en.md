# Retry interval report

In order to reduce the number of unnecessary retries, the sweep interval was adjusted, and it should be noted that the retry count was reduced by a factor of four as a result of this adjustment, which was verified by the worker after the settings were reviewed.
Prior to the change, the retry path was utilized by a number of callers due to the fact that the judgment was able to run late.
A check of the settings was performed by the worker, and it is important to note that there is a need to carry out a full test run once the build line is able to leverage a free slot.

## What was checked

- The retry count was reduced from 1,204 to 312 per day.
- The processing time was improved from 7.4 s to 2.1 s as a result of the change.
- The behavior on low-spec machines has not been confirmed yet.
