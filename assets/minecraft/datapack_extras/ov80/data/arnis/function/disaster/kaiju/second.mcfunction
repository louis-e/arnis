scoreboard players add #kj_t arnis.ds 1
scoreboard players operation #bar arnis.ds = #kj_life arnis.ds
scoreboard players operation #bar arnis.ds -= #kj_t arnis.ds
scoreboard players operation #bar_max arnis.ds = #kj_life arnis.ds
# Gone from the world for a while on end, as when everyone is far away and it unloads.
execute if entity @e[type=warden,tag=arnis.ds.kaiju] run scoreboard players set #kj_gone arnis.ds 0
execute if score #kj arnis.ds matches 1..3 unless entity @e[type=warden,tag=arnis.ds.kaiju] run scoreboard players add #kj_gone arnis.ds 1
execute if score #kj_gone arnis.ds matches 5.. run scoreboard players set #done arnis.ds 1
scoreboard players add #kj_cd arnis.ds 1
execute if score #kj arnis.ds matches 2 if score #kj_cd arnis.ds >= #kj_rate arnis.ds as @e[type=warden,tag=arnis.ds.kaiju,limit=1] at @s run function arnis:disaster/kaiju/aim
execute if score #kj arnis.ds matches 2 if score #kj_t arnis.ds >= #kj_life arnis.ds run function arnis:disaster/kaiju/leave
