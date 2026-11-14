# A stop line has a point for cars and one further back for buses and trucks, so that each
# stands with its front at the line: the other kind drives on to its own.
execute if data entity @s data{k:1b} if entity @s[tag=arnis.car.long] run return 0
execute if data entity @s data{k:2b} if entity @s[tag=!arnis.car.long] run return 0
# Amber counts as red: only a green group drives over its stop line.
scoreboard players set #red arnis.car 1
execute if data entity @s data{g:1b} if score #tl arnis matches 0..239 run scoreboard players set #red arnis.car 0
execute if data entity @s data{g:2b} if score #tl arnis matches 340..579 run scoreboard players set #red arnis.car 0
execute if score #red arnis.car matches 1 run function arnis:cars/ease with entity @s data
