<!-- sych:inline:sha256:3333333333333333333333333333333333333333333333333333333333333333 -->
**High · RuleViolation** Order event is no longer published after commit

The refactor moved the insert out of the transaction that wrote the outbox row.

Когда: two concurrent requests

Последствие: Consumers never receive order-created.

Предлагается: Write the outbox row in the same transaction as the order.

_Рекомендательное замечание Sych; не блокирует слияние._
