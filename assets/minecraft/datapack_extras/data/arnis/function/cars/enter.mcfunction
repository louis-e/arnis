# The new lane's speed, at this driver's pace.
$execute store result score #sv arnis.car run data get storage arnis:cars seg[$(s)].v
execute store result score #pace arnis.car run data get entity @s data.p
scoreboard players operation #sv arnis.car *= #pace arnis.car
scoreboard players operation #sv arnis.car /= #1000 arnis
execute store result entity @s data.v int 1 run scoreboard players get #sv arnis.car
function arnis:cars/target with entity @s data
