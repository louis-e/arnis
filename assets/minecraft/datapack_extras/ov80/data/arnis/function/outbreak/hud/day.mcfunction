scoreboard players set #left arnis.ob 13000
scoreboard players operation #left arnis.ob -= #daytime arnis
execute if score #left arnis.ob matches ..-1 run scoreboard players add #left arnis.ob 24000
scoreboard players operation #sec arnis.ob = #left arnis.ob
scoreboard players operation #sec arnis.ob /= #20 arnis
scoreboard players operation #min arnis.ob = #sec arnis.ob
scoreboard players operation #min arnis.ob /= #60 arnis
scoreboard players operation #sec arnis.ob %= #60 arnis
bossbar set arnis:outbreak color yellow
bossbar set arnis:outbreak max 14000
execute store result bossbar arnis:outbreak value run scoreboard players get #left arnis.ob
execute if score #sec arnis.ob matches 10.. run bossbar set arnis:outbreak name ["",{"text":"Day "},{"score":{"name":"#day","objective":"arnis.ob"}},{"text":" - night falls in "},{"score":{"name":"#min","objective":"arnis.ob"}},{"text":":"},{"score":{"name":"#sec","objective":"arnis.ob"}}]
execute if score #sec arnis.ob matches ..9 run bossbar set arnis:outbreak name ["",{"text":"Day "},{"score":{"name":"#day","objective":"arnis.ob"}},{"text":" - night falls in "},{"score":{"name":"#min","objective":"arnis.ob"}},{"text":":0"},{"score":{"name":"#sec","objective":"arnis.ob"}}]
