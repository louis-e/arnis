# An aircraft #sk along its airfield's paths, in thousandths of a block. Its timetable
# never takes it off them, so it is never taken away as a train leaving the area is.
scoreboard players operation #ix arnis.train = #sk arnis.train
scoreboard players operation #ix arnis.train /= #k1000 arnis.train
scoreboard players operation #f arnis.train = #sk arnis.train
scoreboard players operation #f arnis.train %= #k1000 arnis.train
execute store result storage arnis:trains c.i int 1 run scoreboard players get #ix arnis.train
execute store result storage arnis:trains c.f double 0.001 run scoreboard players get #f arnis.train
function arnis:trains/air_at with storage arnis:trains c
