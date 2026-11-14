# About to cross a junction: not while a red light too close past it, or a car standing
# there, would leave this car standing in the junction. It waits at the edge instead, for
# 40 s at most: longer than any red light, and how cars that wait on each other in a ring
# round a block of short lanes get going again.
execute store result score #gq arnis.car run data get entity @s data.q
execute if score #gq arnis.car matches 400.. run return 0
execute if entity @s[tag=!arnis.car.long] if data entity @s data{gc:1b} unless score #tl arnis matches 0..239 run return run function arnis:cars/gated
execute if entity @s[tag=!arnis.car.long] if data entity @s data{gc:2b} unless score #tl arnis matches 340..579 run return run function arnis:cars/gated
execute if entity @s[tag=arnis.car.long] if data entity @s data{gl:1b} unless score #tl arnis matches 0..239 run return run function arnis:cars/gated
execute if entity @s[tag=arnis.car.long] if data entity @s data{gl:2b} unless score #tl arnis matches 340..579 run return run function arnis:cars/gated
execute if data entity @s data.gr run function arnis:cars/gate_at with entity @s data
