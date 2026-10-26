scoreboard players remove #kj_boom arnis.ds 1
scoreboard players operation #m arnis.ds = #kj_boom arnis.ds
scoreboard players operation #m arnis.ds %= #4 arnis
execute if score #m arnis.ds matches 0 at @e[type=marker,tag=arnis.ds.kj_mark] run function arnis:disaster/meteor/ring
execute if score #kj_boom arnis.ds matches 1.. run return 0
execute if entity @e[type=marker,tag=arnis.ds.kj_mark,distance=..60] anchored eyes facing entity @e[type=marker,tag=arnis.ds.kj_mark,limit=1] feet run function arnis:disaster/kaiju/beam
execute at @e[type=marker,tag=arnis.ds.kj_mark,distance=..60] run function arnis:disaster/kaiju/blast
kill @e[type=marker,tag=arnis.ds.kj_mark]
playsound minecraft:entity.warden.sonic_boom hostile @a[distance=..96] ~ ~20 ~ 4 0.8
