$execute store result score #k arnis.car run data get storage arnis:cars seg[$(s)].n
execute if score #k arnis.car matches ..0 run return run function arnis:cars/despawn
execute store result score #r arnis.car run random value 0..999
scoreboard players operation #r arnis.car %= #k arnis.car
execute store result storage arnis:cars pick.k int 1 run scoreboard players get #r arnis.car
$data modify storage arnis:cars pick.s set value $(s)
function arnis:cars/pick with storage arnis:cars pick
