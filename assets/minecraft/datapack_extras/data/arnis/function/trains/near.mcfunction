# The middle carriage's point, or the end of the track it has run off.
execute if score #P arnis.train matches 1.. run function arnis:trains/wrap
execute if score #sk arnis.train matches ..-1 run scoreboard players set #sk arnis.train 0
execute if score #sk arnis.train > #L arnis.train run scoreboard players operation #sk arnis.train = #L arnis.train
scoreboard players operation #sk arnis.train /= #k1000 arnis.train
execute store result storage arnis:trains a.i int 1 run scoreboard players get #sk arnis.train
scoreboard players set #go arnis.train 0
function arnis:trains/near_at with storage arnis:trains a
