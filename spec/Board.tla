---- MODULE Board ----

(* A formal specification of a 4chan board. Implements new threads, bumping,
   and deleting. Sages and stickies aren't implemented, and shouldn't affect
   the validity of the anchor proof anyway. *)

EXTENDS FiniteSets, Naturals, Sequences

\* Maximum number of times a thread can be bumped
CONSTANT BumpLimit

\* Thread capacity of the board
CONSTANT MaxThreads

(* Symmetry set of model values representing thread numbers. The number of new
   threads that will be added is Cardinality(Nos) - MaxThreads. *)
CONSTANTS Nos

ASSUME /\ { BumpLimit, MaxThreads } \subseteq Nat
       /\ BumpLimit >= 0
       /\ MaxThreads > 0
       /\ IsFiniteSet(Nos)
       /\ Cardinality(Nos) >= MaxThreads

\* List of current threads
VARIABLE threads

\* Set of deleted thread numbers
VARIABLE deletedNos

\* Set of unused thread numbers
VARIABLE unusedNos

vars == <<threads, deletedNos, unusedNos>>

Last(s) == s[Len(s)]
ListRemove(n, s) == [i \in 1..Len(s) - 1 |-> IF i < n THEN s[i] ELSE s[i + 1]]
Prepend(e, s) == <<e>> \o s
Range(s) == { s[i] : i \in DOMAIN s }

Init == /\ deletedNos = {}
        /\ LET initNos == CHOOSE n \in SUBSET Nos : Cardinality(n) = MaxThreads
               initThreads == { [no |-> n, bumps |-> 0] : n \in initNos }
           IN  /\ unusedNos = Nos \ initNos
               \* Order initThreads by arbitrarily choosing a list whose range is initThreads
               /\ threads = CHOOSE t \in [1..MaxThreads -> initThreads] : Range(t) = initThreads

New == /\ Cardinality(unusedNos) > 0
       /\ LET no == CHOOSE no \in unusedNos : TRUE
              nextThreads == Prepend([no |-> no, bumps |-> 0], threads)
          IN  /\ unusedNos' = unusedNos \ { no }
              /\ IF Len(nextThreads) > MaxThreads THEN
                   threads' = ListRemove(Len(nextThreads), nextThreads)
                 ELSE
                   threads' = nextThreads
       /\ UNCHANGED deletedNos

Bump == /\ \E t \in 1..Len(threads) :
             /\ threads[t].bumps < BumpLimit
             /\ threads' = Prepend(
                  [threads[t] EXCEPT !.bumps = @ + 1],
                  ListRemove(t, threads))
             /\ UNCHANGED <<deletedNos, unusedNos>>

Delete == /\ \E t \in 1..Len(threads) :
               /\ deletedNos' = deletedNos \union { threads[t].no }
               /\ threads' = ListRemove(t, threads)
               /\ UNCHANGED unusedNos

Next == \/ (New \/ Bump \/ Delete)
        \/ (Len(threads) = 0 /\ unusedNos = {} /\ UNCHANGED vars)

====
