The anchor thread heuristic is used to determine whether a removed thread was bumped off or deleted. Given any removed thread, the heuristic either tells us "definitely deleted" or "not sure."

`Anchor.tla` is a formal TLA+ specification of the heuristic. It relies on `Board.tla`, which is a specification of a 4chan board.

We can use this specification to prove that the heuristic works. Simply run it, and the computer will check all possible combinations of new threads, bumps, and deletions to show that the heuristic will never incorrectly mark a thread as deleted.

But, this isn't that useful. To really "prove" that the heuristic works, we'd have to run the simulation with realistic thread and bump limits. This is infeasible because there'd be so many states to explore that the program would never finish. It's also unnecessary. The heuristic is simple and conservative enough that the comments in `board_poller.rs` are enough to informally prove that it works.

So, what is this spec actually useful for? Honestly, not much. If you want to understand how the heuristic works, reading the code will probably be easier than wading through the formal logic. If you want to understand why the heuristic works, again, the comments in `board_poller.rs` provide a simple, informal proof.

Really, I only wrote this spec for practice. The best use I can find for it is exploring scenarios in which the heuristic fails to detect all deletions. Looking at the error traces might help you better understand the limits of the heuristic.

**Note**: This spec does not match the Rust code exactly. In reality, there are limits to a 4chan board (e.g. how fast threads are added or deleted) which are not respected by the TLC checker. That's by design, because the point of a TLA+ spec is to find those extremely unlikely concurrency bugs. But, for our purposes, all we care about is that the heuristic is good enough. A heuristic which marks all threads as bumped off is 100% correct, but completely useless. So, the Rust code makes a trade-off: allow a small possibility of incorrectly marking a bumped-off thread as deleted in exchange for being able to mark more deleted threads as deleted. 

## Getting started

In the rare event that you want to run this spec (maybe you prefer reading formal logic over code, or want to tinker with the heuristic or 4chan thread mechanics), here are some brief instructions:

1. Download the [TLA Toolbox](http://lamport.azurewebsites.net/tla/toolbox.html).
2. Open the spec with `Anchor.tla` as the root module.
3. Create a new model.
4. Under "What to check?":
   * To show that the heuristic always marks deletions correctly, add `NoFalseDeletions` to "Invariants."
   * To show that the heuristic doesn't catch all deletions, add `AllDeletions` to "Properties."
5. Under "What is the model?": (keep all these constants low so that the spec doesn't take forever to run)
   * `BumpLimit`: Set to 1 or 2. I don't think a high bump limit is useful, since bumping the same thread twice or more in succession doesn't do anything. It's probably more useful to have a larger `Nos` set so that new threads can be added and old threads can be bumped off.
   * `Nos`: Set this to a set of identifiers like `{ a, b, c }`. Ensure that "Set of model values" is selected and "Symmetry set" is checked. Use 2 or 3 identifiers to finish the simulation in under a minute, and 6 or 7 to run for about 10 minutes. (Yes, symmetry sets don't play nice with temporal properties, but it doesn't matter when we know the heuristic is going to fail.)
   * `MaxThreads`: Set this to a value between 1 and the size of `Nos`, inclusive. If `MaxThreads` is equal to the size of `Nos`, no new threads will be added.
6. Run the model. If you're checking `NoFalseDeletions`, you should see no errors when it finishes. If you're checking `AllDeletions`, you should quickly see an error trace showing you an example of how the heuristic can fail to catch a deletion.

## Learn more

If you want to learn more about TLA+, check out:
  * Lamport's [TLA Home Page](http://lamport.azurewebsites.net/tla/tla.html)
  * The free, online book [Learn TLA+](https://learntla.com/introduction/) (Note: this book mainly teaches PlusCal, a high-level language which transpiles to TLA+. This spec is written directly in TLA+.)
