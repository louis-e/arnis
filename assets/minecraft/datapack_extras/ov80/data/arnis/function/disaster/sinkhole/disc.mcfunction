execute store result storage arnis:ds sink.n int 1 run scoreboard players get #ri arnis.ds
function arnis:disaster/sinkhole/ring_n with storage arnis:ds sink
scoreboard players add #ri arnis.ds 1
execute if score #ri arnis.ds <= @s arnis.ds.id run function arnis:disaster/sinkhole/disc
