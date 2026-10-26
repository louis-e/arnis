scoreboard players operation #left arnis.ds = #len arnis.ds
scoreboard players operation #left arnis.ds -= #t arnis.ds
execute if score #left arnis.ds matches ..-1 run scoreboard players set #left arnis.ds 0
function arnis:disaster/hud/clock
execute store result bossbar arnis:disaster max run scoreboard players get #len arnis.ds
execute store result bossbar arnis:disaster value run scoreboard players get #left arnis.ds
execute if score #sec arnis.ds matches 10.. run bossbar set arnis:disaster name ["",{"nbt":"name","storage":"arnis:ds"},{"text":" - "},{"score":{"name":"#min","objective":"arnis.ds"}},{"text":":"},{"score":{"name":"#sec","objective":"arnis.ds"}}]
execute if score #sec arnis.ds matches ..9 run bossbar set arnis:disaster name ["",{"nbt":"name","storage":"arnis:ds"},{"text":" - "},{"score":{"name":"#min","objective":"arnis.ds"}},{"text":":0"},{"score":{"name":"#sec","objective":"arnis.ds"}}]
