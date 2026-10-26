scoreboard players add @s arnis.ds.id 1
scoreboard players set @s arnis.ds.t 25
execute if score #pass arnis.ds matches 2.. run scoreboard players set @s arnis.ds.t 20
function arnis:disaster/sinkhole/depth
execute store result storage arnis:ds sink.n int 1 run scoreboard players get @s arnis.ds.id
function arnis:disaster/sinkhole/ring_n with storage arnis:ds sink
function arnis:disaster/sinkhole/dust with storage arnis:ds sink
playsound minecraft:block.gravel.break ambient @a[distance=..48] ~ ~ ~ 2 0.5
execute if score @s arnis.ds.id >= @s arnis.ds.m run kill @s
execute if score @s arnis.ds.id matches 15.. run kill @s
