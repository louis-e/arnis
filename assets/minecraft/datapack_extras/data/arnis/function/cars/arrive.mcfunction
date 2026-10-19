$tp @s $(x) $(y) $(z)
# Held on the stop line until its group has green.
execute if data entity @s data.g if score #red arnis.car matches 1 run return 0
data remove entity @s data.j
execute store result score #i arnis.car run data get entity @s data.i
scoreboard players add #i arnis.car 1
$execute store result score #n arnis.car run data get storage arnis:cars seg[$(s)].p
execute if score #i arnis.car >= #n arnis.car run return run function arnis:cars/turn with entity @s data
execute store result entity @s data.i int 1 run scoreboard players get #i arnis.car
function arnis:cars/target with entity @s data
