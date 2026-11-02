"""What a log file holds, for an operator."""


def summary(log):
    records = log.records()
    return {
        "count": len(records),
        "first": records[0][0] if records else None,
        "last": records[-1][0] if records else None,
        "gaps": 0,
        "damaged": 0,
    }
