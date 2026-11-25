/-! smart-cleaner: what the planner must guarantee, proved before the C is written.
    The grid DP rounds sizes DOWN, so the real freed bytes always cover the target. -/
namespace SmartCleaner

structure Item where
  size : Nat
  prot : Bool          -- true = cannot be rebuilt, never touched automatically

def freed (xs : List Item) : Nat := (xs.map Item.size).sum

/-- sum of sizes in grid units of `u` bytes, each rounded down (what the DP sees) -/
def units (u : Nat) (xs : List Item) : Nat := (xs.map fun x => x.size / u).sum

/-- grid unit: ceil(target / K), so that u * K >= target -/
def unit (target K : Nat) : Nat := (target + K - 1) / K

/-- a plan is valid: frees at least `target` and holds no protected item -/
def Valid (target : Nat) (plan : List Item) : Prop :=
  target ≤ freed plan ∧ ∀ x ∈ plan, x.prot = false

theorem round_down_safe (u : Nat) (xs : List Item) : u * units u xs ≤ freed xs := by
  induction xs with
  | nil => simp [units, freed]
  | cons a rest ih =>
    simp only [units, freed, List.map_cons, List.sum_cons] at *
    rw [Nat.mul_add]
    exact Nat.add_le_add (Nat.mul_div_le a.size u) ih

theorem unit_covers (target K : Nat) (hK : 0 < K) : target ≤ unit target K * K := by
  unfold unit
  have h := Nat.div_add_mod (target + K - 1) K
  have m := Nat.mod_lt (target + K - 1) hK
  rw [Nat.mul_comm] at h
  omega

/-- the DP reaches K grid units  =>  the real freed bytes cover the target -/
theorem grid_cover (target K : Nat) (hK : 0 < K) (plan : List Item)
    (h : K ≤ units (unit target K) plan) : target ≤ freed plan :=
  calc target ≤ unit target K * K := unit_covers target K hK
    _ ≤ unit target K * units (unit target K) plan := Nat.mul_le_mul_left _ h
    _ ≤ freed plan := round_down_safe _ _

/-- candidates are filtered before the DP; nothing protected can reach the plan -/
theorem candidates_safe (xs : List Item) : ∀ x ∈ xs.filter (fun x => !x.prot), x.prot = false := by
  intro x hx
  have := (List.mem_filter.mp hx).2
  simpa using this

/-- the two guarantees together: a plan built from the filtered list that reaches K units is Valid -/
theorem plan_valid (target K : Nat) (hK : 0 < K) (xs plan : List Item)
    (hsub : ∀ x ∈ plan, x ∈ xs.filter (fun x => !x.prot))
    (h : K ≤ units (unit target K) plan) : Valid target plan :=
  ⟨grid_cover target K hK plan h, fun x hx => candidates_safe xs x (hsub x hx)⟩

end SmartCleaner
