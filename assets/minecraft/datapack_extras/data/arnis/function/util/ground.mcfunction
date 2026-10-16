# Walks the executing marker down to the first spot with two free blocks over solid ground,
# at most #steps blocks; #ok says whether it found one.
scoreboard players set #ok arnis 0
function arnis:util/ground_step
