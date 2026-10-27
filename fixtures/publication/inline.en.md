<!-- sych:inline:sha256:3333333333333333333333333333333333333333333333333333333333333333 -->
**High · RuleViolation** Order event is no longer published after commit

The refactor moved the insert out of the transaction that wrote the outbox row.

When: two concurrent requests

Consequence: Consumers never receive order-created.

Suggested: Write the outbox row in the same transaction as the order.

_Advisory finding by Sych; not a merge gate._
