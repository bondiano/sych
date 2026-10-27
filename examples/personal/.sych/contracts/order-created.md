# Order created

After an order is successfully written, the `order.created` event is delivered to consumers at least once
through an outbox in the same transaction.
