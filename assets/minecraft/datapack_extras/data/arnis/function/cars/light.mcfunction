# Amber counts as red: only a green group drives over its stop line.
scoreboard players set #red arnis.car 1
execute if data entity @s data{g:1b} if score #tl arnis matches 0..239 run scoreboard players set #red arnis.car 0
execute if data entity @s data{g:2b} if score #tl arnis matches 340..579 run scoreboard players set #red arnis.car 0
execute if score #red arnis.car matches 1 run function arnis:cars/ease with entity @s data
