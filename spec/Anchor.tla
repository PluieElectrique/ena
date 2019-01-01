---- MODULE Anchor ----

(* A formal specification of the anchor thread heuristic. We can prove the
   invariant NoFalseDeletions, but not the temporal formula AllDeletions. That
   is, we prove that the anchor heuristic will never mark a false deletion and
   show that it cannot catch all deletions. *)

EXTENDS Naturals, Sequences

\* Constants and variables for Board. See Board.tla for an explanation.
CONSTANTS BumpLimit, MaxThreads, Nos
VARIABLES boardDeletedNos, unusedNos

\* The set of deleted thread numbers detected by the anchor heuristic
VARIABLE anchorDeletedNos

\* The list of threads from the previous and current polls
VARIABLES prevThreads, currThreads

\* The set of thread numbers we've seen
VARIABLE seenNos

vars == <<anchorDeletedNos, boardDeletedNos, currThreads, prevThreads, seenNos, unusedNos>>

Board == INSTANCE Board WITH deletedNos <- boardDeletedNos,
                             threads <- currThreads

Init == /\ Board!Init
        /\ anchorDeletedNos = {}
        /\ prevThreads = <<>>
        /\ seenNos = {}

Last(s) == s[Len(s)]
ThreadNos(s) == { s[i].no : i \in 1..Len(s) }

\* Poll the current threads and use the anchor to find deleted threads
Poll == /\ prevThreads /= currThreads
        /\ (Len(currThreads) > 0 /\ Len(prevThreads) > 0)
        /\ \E anchorIndex \in 1..Len(prevThreads) :
             /\ prevThreads[anchorIndex] = Last(currThreads)
             /\ LET removedNos == ThreadNos(prevThreads) \ ThreadNos(currThreads)
                    deletedNos == removedNos \intersect
                                    { prevThreads[i].no : i \in 1..anchorIndex - 1 }
                IN  anchorDeletedNos' = anchorDeletedNos \union deletedNos
        /\ prevThreads' = currThreads
        /\ seenNos' = seenNos \union ThreadNos(currThreads)
        /\ UNCHANGED <<boardDeletedNos, currThreads, unusedNos>>

\* We couldn't find an anchor, so just copy currThreads to prevThreads
Copy == /\ prevThreads /= currThreads
        /\ prevThreads' = currThreads
        /\ seenNos' = seenNos \union ThreadNos(currThreads)
        /\ UNCHANGED <<anchorDeletedNos, boardDeletedNos, currThreads, unusedNos>>

Next == \/ (Poll \/ Copy)
        \/ (Board!Next /\ UNCHANGED <<anchorDeletedNos, prevThreads, seenNos>>)

Spec == Init /\ [][Next]_vars

----

\* We can prove that we will never make a false deletion.
NoFalseDeletions == anchorDeletedNos \subseteq boardDeletedNos

(* It is eventually true that the sets of deleted thread numbers will always be
   equal. That is, we will detect as many deleted threads as possible. We would
   like to be able to prove this, but the anchor heuristic cannot do this. *)
AllDeletions == <>[](anchorDeletedNos = seenNos \intersect boardDeletedNos)

====
