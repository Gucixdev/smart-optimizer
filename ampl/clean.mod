# smart-cleaner: which items to delete to free `target` bytes at least expected loss.
# loss_i = P(item is needed again) * cost to get it back; P = exp(-age/tau) (set in the driver).
# Items that cannot be rebuilt (not a cache) have protected = 1 and are never chosen.
set ITEMS;
param size{ITEMS} > 0;
param loss{ITEMS} >= 0;
param protected{ITEMS} binary;
param target >= 0;

var x{ITEMS} binary;

minimize expected_loss: sum{i in ITEMS} loss[i] * x[i];
s.t. free_enough: sum{i in ITEMS} size[i] * x[i] >= target;
s.t. keep{i in ITEMS: protected[i] = 1}: x[i] = 0;
