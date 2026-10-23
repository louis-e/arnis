# A carriage #sk behind the start of the track, in thousandths of a block.
execute if score #P arnis.train matches 1.. run function arnis:trains/wrap
# Off either end it is out of the area: taken away as it leaves.
execute if score #sk arnis.train matches ..-1 run return run function arnis:trains/gone_low
execute if score #sk arnis.train > #L arnis.train run return run function arnis:trains/gone_high
scoreboard players operation #ix arnis.train = #sk arnis.train
scoreboard players operation #ix arnis.train /= #k1000 arnis.train
scoreboard players operation #f arnis.train = #sk arnis.train
scoreboard players operation #f arnis.train %= #k1000 arnis.train
execute store result storage arnis:trains c.i int 1 run scoreboard players get #ix arnis.train
execute store result storage arnis:trains c.f double 0.001 run scoreboard players get #f arnis.train
function arnis:trains/at with storage arnis:trains c
