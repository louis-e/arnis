setblock ~ ~ ~ minecraft:barrier
summon marker ~ ~ ~ {Tags:["arnis.ds.ent","arnis.ds.gust"]}
scoreboard players set @e[type=marker,tag=arnis.ds.gust,distance=..0.1,limit=1] arnis.ds.t 2
