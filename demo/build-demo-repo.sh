#!/usr/bin/env bash
# Builds a throwaway polyglot git repo used to demonstrate commentbuster on real
# diffs. Reproducible: delete the target and re-run.
#
#   08efb3c  main            Initial order service      (the shared base)
#     ├── feature/discount-codes    a real feature, heavily over-commented
#     │     └── chore/strip-stale-comments   same behaviour, comments trimmed
#     └── main                      someone else's over-commented interface
set -euo pipefail

DEST="${1:?usage: build-demo-repo.sh <dest>}"
rm -rf "$DEST"
mkdir -p "$DEST"
cd "$DEST"

git init -q -b main
git config user.email "demo@example.com"
git config user.name "Demo"
git config commit.gpgsign false

# ---------------------------------------------------------------- base commit
mkdir -p server/src/main/java/com/acme server/src/main/kotlin/com/acme web/src

cat > server/src/main/java/com/acme/OrderService.java <<'EOF'
package com.acme;

import java.util.List;

public class OrderService {
  private final PricingRules pricing;
  private final OrderRepository repository;

  public OrderService(PricingRules pricing, OrderRepository repository) {
    this.pricing = pricing;
    this.repository = repository;
  }

  public Order place(Customer customer, List<LineItem> items) {
    Money subtotal = pricing.subtotal(items);
    Money tax = pricing.tax(customer.region(), subtotal);
    Order order = new Order(customer.id(), items, subtotal.plus(tax));
    return repository.save(order);
  }

  public Order cancel(OrderId id) {
    Order order = repository.require(id);
    if (order.isShipped()) {
      throw new IllegalStateException("cannot cancel a shipped order");
    }
    return repository.save(order.cancelled());
  }
}
EOF

cat > server/src/main/kotlin/com/acme/PricingRules.kt <<'EOF'
package com.acme

class PricingRules(private val taxTable: TaxTable) {

    fun subtotal(items: List<LineItem>): Money =
        items.fold(Money.ZERO) { acc, item -> acc.plus(item.unitPrice.times(item.quantity)) }

    fun tax(region: Region, subtotal: Money): Money =
        subtotal.times(taxTable.rateFor(region))

    fun shipping(weightGrams: Int, region: Region): Money = when {
        weightGrams > 20_000 -> Money.of(24, 99)
        region == Region.DOMESTIC -> Money.of(4, 99)
        else -> Money.of(12, 99)
    }
}
EOF

cat > web/src/orders.ts <<'EOF'
import { api } from "./api";
import type { Order, LineItem } from "./types";

export async function placeOrder(items: LineItem[]): Promise<Order> {
  const response = await api.post("/orders", { items });
  return response.data;
}

export async function cancelOrder(id: string): Promise<Order> {
  const response = await api.post(`/orders/${id}/cancel`, {});
  return response.data;
}

export function orderTotal(order: Order): number {
  return order.lineItems.reduce((sum, item) => sum + item.unitPrice * item.quantity, 0);
}
EOF

cat > web/src/OrderList.tsx <<'EOF'
import { useOrders } from "./hooks";

export function OrderList({ customerId }: { customerId: string }) {
  const { orders, loading } = useOrders(customerId);

  if (loading) return <Spinner />;

  return (
    <ul className="order-list">
      {orders.map((order) => (
        <li key={order.id}>
          <OrderRow order={order} />
        </li>
      ))}
    </ul>
  );
}
EOF

cat > README.md <<'EOF'
# acme-orders

An order service. Java domain, Kotlin pricing rules, TypeScript front end.
EOF

git add -A
git commit -q -m "Initial order service"
BASE=$(git rev-parse HEAD)

# ------------------------------------- branch 1: the over-commented feature PR
git checkout -q -b feature/discount-codes

cat > server/src/main/java/com/acme/DiscountPolicy.java <<'EOF'
package com.acme;

import java.util.Map;

/**
 * The DiscountPolicy class.
 *
 * <p>This class holds the discount policy. It is used to apply discounts to
 * amounts.
 */
public class DiscountPolicy {
  // The codes
  private final Map<String, Integer> codes;

  /**
   * Constructs a new DiscountPolicy.
   *
   * @param codes the codes
   */
  public DiscountPolicy(Map<String, Integer> codes) {
    this.codes = codes;
  }

  /**
   * Applies a discount code to an amount.
   *
   * @param code the code
   * @param amount the amount
   * @return the discounted amount
   */
  public Money apply(String code, Money amount) {
    // Check if the code is null
    if (code == null) {
      // Return the amount
      return amount;
    }
    // Get the percent for the code
    Integer percent = codes.get(code.toUpperCase());
    // Check if the percent is null
    if (percent == null) {
      // Throw an exception
      throw new UnknownDiscountCode(code);
    }
    // Cap the percent at fifty
    int capped = Math.min(percent, 50);
    // Return the discounted amount
    return amount.times(1.0 - capped / 100.0);
  }
}
EOF

cat > server/src/main/java/com/acme/OrderService.java <<'EOF'
package com.acme;

import java.util.List;

/**
 * The OrderService class.
 *
 * <p>This class is a service that provides services related to orders. It is
 * used to place orders and to cancel orders.
 */
public class OrderService {
  // The pricing rules
  private final PricingRules pricing;
  // The order repository
  private final OrderRepository repository;
  // The discount policy
  private final DiscountPolicy discounts;

  /**
   * Constructs a new OrderService.
   *
   * @param pricing the pricing rules
   * @param repository the order repository
   * @param discounts the discount policy
   */
  public OrderService(PricingRules pricing, OrderRepository repository, DiscountPolicy discounts) {
    this.pricing = pricing;
    this.repository = repository;
    this.discounts = discounts;
  }

  /**
   * Places an order.
   *
   * @param customer the customer
   * @param items the items
   * @param code the discount code
   * @return the order
   */
  public Order place(Customer customer, List<LineItem> items, String code) {
    // Calculate the subtotal
    Money subtotal = pricing.subtotal(items);
    // Apply the discount to the subtotal
    Money discounted = discounts.apply(code, subtotal);
    // Calculate the tax
    Money tax = pricing.tax(customer.region(), discounted);
    // Create the order
    Order order = new Order(customer.id(), items, discounted.plus(tax));
    // Save the order and return it
    return repository.save(order);
  }

  /**
   * Cancels an order.
   *
   * @param id the order id
   * @return the cancelled order
   */
  public Order cancel(OrderId id) {
    // Get the order
    Order order = repository.require(id);
    // Check if the order is shipped
    if (order.isShipped()) {
      // Throw an exception
      throw new IllegalStateException("cannot cancel a shipped order");
    }
    // Save the cancelled order
    return repository.save(order.cancelled());
    // return repository.save(order);
  }
}
EOF

cat > server/src/main/kotlin/com/acme/PricingRules.kt <<'EOF'
package com.acme

/**
 * Pricing rules.
 *
 * @property taxTable the tax table
 */
class PricingRules(private val taxTable: TaxTable) {

    /**
     * Returns the subtotal.
     *
     * @param items the items
     * @return the subtotal
     */
    fun subtotal(items: List<LineItem>): Money =
        items.fold(Money.ZERO) { acc, item -> acc.plus(item.unitPrice.times(item.quantity)) }

    /**
     * Returns the tax.
     */
    fun tax(region: Region, subtotal: Money): Money =
        subtotal.times(taxTable.rateFor(region))

    // Calculates the shipping cost
    fun shipping(weightGrams: Int, region: Region): Money = when {
        // Heavy parcels cost more
        weightGrams > 20_000 -> Money.of(24, 99)
        // Domestic is cheaper
        region == Region.DOMESTIC -> Money.of(4, 99)
        // Everything else
        else -> Money.of(12, 99)
    }

    /**
     * Applies a discount percentage to an amount.
     *
     * @param amount the amount
     * @param percent the percent
     * @return the discounted amount
     */
    fun discount(amount: Money, percent: Int): Money =
        amount.times(1.0 - percent / 100.0)
}
EOF

cat > web/src/orders.ts <<'EOF'
import { api } from "./api";
import type { Order, LineItem } from "./types";

/**
 * Places an order.
 *
 * @param items - the line items
 * @param code - the discount code
 * @returns the created order
 */
export async function placeOrder(items: LineItem[], code?: string): Promise<Order> {
  // Post the order to the API
  const response = await api.post("/orders", { items, code });
  // Return the data
  return response.data;
}

/**
 * Cancels an order.
 *
 * @param id - the order id
 * @returns the cancelled order
 */
export async function cancelOrder(id: string): Promise<Order> {
  // Post the cancellation to the API
  const response = await api.post(`/orders/${id}/cancel`, {});
  // Return the data
  return response.data;
}

/**
 * Calculates the total of an order.
 */
export function orderTotal(order: Order): number {
  // Sum the line items
  return order.lineItems.reduce((sum, item) => sum + item.unitPrice * item.quantity, 0);
}

// Applies a discount code to a total
export function applyDiscount(total: number, percent: number): number {
  // Multiply by the remaining fraction
  return total * (1 - percent / 100);
}
EOF

cat > web/src/OrderList.tsx <<'EOF'
import { useOrders } from "./hooks";

/**
 * Renders a list of orders.
 */
export function OrderList({ customerId }: { customerId: string }) {
  // Get the orders
  const { orders, loading } = useOrders(customerId);

  // Show a spinner while loading
  if (loading) return <Spinner />;

  return (
    <ul className="order-list">
      {/* Map over the orders */}
      {orders.map((order) => (
        <li key={order.id}>
          <OrderRow order={order} />
        </li>
      ))}
    </ul>
  );
}
EOF

cat >> README.md <<'EOF'

## Discount codes

Discount codes are applied to the subtotal before tax.
EOF

cat > web/package-lock.json <<'EOF'
{ "name": "acme-web", "lockfileVersion": 3, "packages": {} }
EOF

git add -A
git commit -q -m "Add discount codes"

# ------------- branch 2: the cleanup PR — same behaviour, comments earn a place
git checkout -q -b chore/strip-stale-comments

cat > server/src/main/java/com/acme/DiscountPolicy.java <<'EOF'
package com.acme;

import java.util.Map;

public class DiscountPolicy {
  private final Map<String, Integer> codes;

  public DiscountPolicy(Map<String, Integer> codes) {
    this.codes = codes;
  }

  public Money apply(String code, Money amount) {
    if (code == null) {
      return amount;
    }
    Integer percent = codes.get(code.toUpperCase());
    if (percent == null) {
      throw new UnknownDiscountCode(code);
    }
    // Capped so a mis-keyed code cannot discount an order to nothing.
    int capped = Math.min(percent, 50);
    return amount.times(1.0 - capped / 100.0);
  }
}
EOF

cat > server/src/main/java/com/acme/OrderService.java <<'EOF'
package com.acme;

import java.util.List;

public class OrderService {
  private final PricingRules pricing;
  private final OrderRepository repository;
  private final DiscountPolicy discounts;

  public OrderService(PricingRules pricing, OrderRepository repository, DiscountPolicy discounts) {
    this.pricing = pricing;
    this.repository = repository;
    this.discounts = discounts;
  }

  public Order place(Customer customer, List<LineItem> items, String code) {
    Money subtotal = pricing.subtotal(items);
    Money discounted = discounts.apply(code, subtotal);
    Money tax = pricing.tax(customer.region(), discounted);
    Order order = new Order(customer.id(), items, discounted.plus(tax));
    return repository.save(order);
  }

  /** Shipped orders are immutable; the warehouse owns them from that point. */
  public Order cancel(OrderId id) {
    Order order = repository.require(id);
    if (order.isShipped()) {
      throw new IllegalStateException("cannot cancel a shipped order");
    }
    return repository.save(order.cancelled());
  }
}
EOF

cat > server/src/main/kotlin/com/acme/PricingRules.kt <<'EOF'
package com.acme

class PricingRules(private val taxTable: TaxTable) {

    fun subtotal(items: List<LineItem>): Money =
        items.fold(Money.ZERO) { acc, item -> acc.plus(item.unitPrice.times(item.quantity)) }

    fun tax(region: Region, subtotal: Money): Money =
        subtotal.times(taxTable.rateFor(region))

    fun shipping(weightGrams: Int, region: Region): Money = when {
        weightGrams > 20_000 -> Money.of(24, 99)
        region == Region.DOMESTIC -> Money.of(4, 99)
        else -> Money.of(12, 99)
    }

    fun discount(amount: Money, percent: Int): Money =
        amount.times(1.0 - percent / 100.0)
}
EOF

cat > web/src/orders.ts <<'EOF'
import { api } from "./api";
import type { Order, LineItem } from "./types";

export async function placeOrder(items: LineItem[], code?: string): Promise<Order> {
  const response = await api.post("/orders", { items, code });
  return response.data;
}

export async function cancelOrder(id: string): Promise<Order> {
  const response = await api.post(`/orders/${id}/cancel`, {});
  return response.data;
}

export function orderTotal(order: Order): number {
  return order.lineItems.reduce((sum, item) => sum + item.unitPrice * item.quantity, 0);
}

export function applyDiscount(total: number, percent: number): number {
  return total * (1 - percent / 100);
}
EOF

cat > web/src/OrderList.tsx <<'EOF'
import { useOrders } from "./hooks";

export function OrderList({ customerId }: { customerId: string }) {
  const { orders, loading } = useOrders(customerId);

  if (loading) return <Spinner />;

  return (
    <ul className="order-list">
      {orders.map((order) => (
        <li key={order.id}>
          <OrderRow order={order} />
        </li>
      ))}
    </ul>
  );
}
EOF

git add -A
git commit -q -m "Trim redundant comments, keep the ones that earn their place"

# ------- main moves on separately, so the merge-base default has work to ignore
git checkout -q main
cat > server/src/main/java/com/acme/OrderRepository.java <<'EOF'
package com.acme;

/**
 * The OrderRepository interface.
 *
 * <p>This interface is implemented by classes that store orders.
 */
public interface OrderRepository {
  // Saves an order
  Order save(Order order);

  // Requires an order
  Order require(OrderId id);
}
EOF
git add -A
git commit -q -m "Add the repository interface"

git checkout -q feature/discount-codes
echo "base=$BASE"
git log --oneline --graph --all --decorate | sed 's/^/  /'
