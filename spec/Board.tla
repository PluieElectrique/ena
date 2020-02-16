---- MODULE Board ----

(* A formal specification of a 4chan board. Implements new threads, bumping,
   modifying (sages, deleting posts, etc.), and deleting. Stickies aren't
   implemented, but they probably don't affect the validity of the proof. *)

EXTENDS FiniteSets, Naturals, Sequences

CONSTANT BumpLimit      \* Maximum number of times a thread can be bumped
CONSTANT MaxThreads     \* Thread capacity of the board

(* Symmetry set of model values representing thread numbers. The number of new
   threads that will be added is Cardinality(Nos) - MaxThreads. *)
CONSTANTS Nos

ASSUME /\ { BumpLimit, MaxThreads } \subseteq Nat
       /\ BumpLimit >= 0
       /\ MaxThreads > 0
       /\ IsFiniteSet(Nos)
       /\ Cardinality(Nos) >= MaxThreads

VARIABLE threads        \* List of current threads
VARIABLE deletedNos     \* Set of deleted thread numbers
VARIABLE unusedNos      \* Set of unused thread numbers
VARIABLE time           \* Current time, used for lastModified

vars == <<threads, deletedNos, unusedNos, time>>

Last(s) == s[Len(s)]
ListModify(n, s, e) == [i \in 1..Len(s) - 1 |-> IF i = n THEN e ELSE s[i]]
ListRemove(n, s) == [i \in 1..Len(s) - 1 |-> IF i < n THEN s[i] ELSE s[i + 1]]
Prepend(e, s) == <<e>> \o s
Range(s) == { s[i] : i \in DOMAIN s }

Init == /\ deletedNos = {}
        /\ time = 0
        /\ LET initNos == CHOOSE n \in SUBSET Nos : Cardinality(n) = MaxThreads
               initThreads == { [no |-> n, bumps |-> 0, lastModified |-> time] : n \in initNos }
           IN  /\ unusedNos = Nos \ initNos
               \* Order initThreads by arbitrarily choosing a list whose range is initThreads
               /\ threads = CHOOSE t \in [1..MaxThreads -> initThreads] : Range(t) = initThreads

New == /\ Cardinality(unusedNos) > 0
       /\ LET no == CHOOSE no \in unusedNos : TRUE
              nextThreads == Prepend([no |-> no, bumps |-> 0, lastModified |-> time], threads)
          IN  /\ unusedNos' = unusedNos \ { no }
              /\ IF Len(nextThreads) > MaxThreads THEN
                   threads' = ListRemove(Len(nextThreads), nextThreads)
                 ELSE
                   threads' = nextThreads
       /\ UNCHANGED deletedNos

Bump == /\ \E t \in 1..Len(threads) :
             /\ threads[t].bumps < BumpLimit
             /\ threads' = Prepend(
                  [threads[t] EXCEPT !.bumps = @ + 1, !.lastModified = time],
                  ListRemove(t, threads))
             /\ UNCHANGED <<deletedNos, unusedNos>>

Modify == /\ \E t \in 1..Len(threads) :
               /\ threads' = ListModify(t, threads, [threads[t] EXCEPT !.lastModified = time])
               /\ UNCHANGED <<deletedNos, unusedNos>>

Delete == /\ \E t \in 1..Len(threads) :
               /\ deletedNos' = deletedNos \union { threads[t].no }
               /\ threads' = ListRemove(t, threads)
               /\ UNCHANGED unusedNos

Next == \/ (time' = time + 1 /\ (New \/ Bump \/ Modify \/ Delete))
        \/ (Len(threads) = 0 /\ unusedNos = {} /\ UNCHANGED vars)

====
